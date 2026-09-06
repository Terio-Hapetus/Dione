//! Pure worktree helpers (M2 core, no git side effects yet).
//!
//! Conventions (cf. Orca / Hermes `-w` / Codex App):
//! - path: `<repo>/.ade-worktrees/<slug>`
//! - branch: `ade/<slug>`
//! - max ~15 managed worktrees, prune stale on startup.

use std::path::{Path, PathBuf};

pub const WORKTREE_DIR_NAME: &str = ".ade-worktrees";
pub const BRANCH_PREFIX: &str = "ade/";
pub const MAX_MANAGED_WORKTREES: usize = 15;
/// Optional file in the repo root listing gitignored paths (one per line)
/// to copy into fresh worktrees (cf. Hermes/Cline `.worktreeinclude`).
pub const WORKTREE_INCLUDE_FILE: &str = ".worktreeinclude";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeStatus {
    Creating,
    Working,
    NeedsYou,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRecord {
    pub slug: String,
    pub branch: String,
    pub path: PathBuf,
    pub status: WorktreeStatus,
    /// Main opencode session driving this worktree, if any.
    pub session_id: Option<String>,
}

impl WorktreeRecord {
    pub fn new(repo: &Path, slug: &str) -> Option<Self> {
        let slug = normalize_slug(slug);
        if slug.is_empty() {
            return None;
        }
        Some(Self {
            branch: branch_name(&slug),
            path: worktree_path(repo, &slug),
            slug,
            status: WorktreeStatus::Creating,
            session_id: None,
        })
    }
}

/// Lowercase, alphanumeric + `-`, collapse runs, trim dashes, max 48 chars.
pub fn normalize_slug(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = true; // trim leading dashes
    for c in raw.chars().flat_map(|c| c.to_lowercase()) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= 48 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

pub fn branch_name(slug: &str) -> String {
    format!("{BRANCH_PREFIX}{slug}")
}

pub fn worktree_path(repo: &Path, slug: &str) -> PathBuf {
    repo.join(WORKTREE_DIR_NAME).join(slug)
}

pub fn is_worktree_path(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| s == WORKTREE_DIR_NAME)
    })
}

// ------------------------------------------------------------ git ops

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("invalid worktree slug: {0:?}")]
    InvalidSlug(String),
    #[error("worktree limit reached ({MAX_MANAGED_WORKTREES})")]
    LimitReached,
    #[error("worktree already exists: {0}")]
    AlreadyExists(String),
    #[error("{0}")]
    Git(String),
    #[error("io error: {0}")]
    Io(String),
}

/// One entry of `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    /// `refs/heads/<name>` short name, if on a branch.
    pub branch: Option<String>,
    pub detached: bool,
}

/// Create a worktree at `<repo>/.ade-worktrees/<slug>` on branch
/// `ade/<slug>`, then copy `.worktreeinclude` entries into it.
pub async fn create(repo: &Path, raw_slug: &str) -> Result<WorktreeRecord, WorktreeError> {
    let slug = normalize_slug(raw_slug);
    if slug.is_empty() {
        return Err(WorktreeError::InvalidSlug(raw_slug.to_string()));
    }
    if managed_count(repo).await? >= MAX_MANAGED_WORKTREES {
        return Err(WorktreeError::LimitReached);
    }
    let path = worktree_path(repo, &slug);
    if path.exists() {
        return Err(WorktreeError::AlreadyExists(slug));
    }
    let branch = branch_name(&slug);
    run_git(
        repo,
        &["worktree", "add", &path.to_string_lossy(), "-b", &branch],
    )
    .await?;
    copy_worktreeinclude(repo, &path);
    Ok(WorktreeRecord {
        slug,
        branch,
        path,
        status: WorktreeStatus::Creating,
        session_id: None,
    })
}

/// Remove a worktree and force-delete its branch. Keeps going when parts
/// are already gone (crashed sessions); errors only on real git failures.
pub async fn remove(repo: &Path, slug: &str) -> Result<(), WorktreeError> {
    let path = worktree_path(repo, slug);
    if path.exists() || is_registered(repo, &path).await {
        // `--force` handles dirty worktrees; a missing path is not fatal.
        let _ = run_git(
            repo,
            &["worktree", "remove", "--force", &path.to_string_lossy()],
        )
        .await;
    }
    // Best effort: drop our branch if nothing checks it out anymore.
    if !branch_checked_out(repo, slug).await? {
        let _ = run_git(repo, &["branch", "-D", &branch_name(slug)]).await;
    }
    Ok(())
}

/// All worktrees registered in `repo` (main checkout first).
pub async fn list(repo: &Path) -> Result<Vec<WorktreeInfo>, WorktreeError> {
    parse_porcelain(&run_git(repo, &["worktree", "list", "--porcelain"]).await?)
}

/// Agent-agnostic diff of one checkout vs its HEAD, via git directly
/// (no `/session/diff`). `raw` covers tracked changes; `files` lists every
/// dirty path including untracked ones (from `status --porcelain`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GitDiff {
    pub files: Vec<String>,
    pub raw: String,
}

impl GitDiff {
    pub fn is_empty(&self) -> bool {
        self.raw.trim().is_empty() && self.files.is_empty()
    }

    /// Same shape as the legacy `diffs` map values, plus `source: "git"`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "source": "git", "files": self.files, "raw": self.raw })
    }
}

pub async fn git_diff(path: &Path) -> Result<GitDiff, WorktreeError> {
    let raw = run_git(path, &["diff", "HEAD", "--"]).await?;
    let status = run_git(path, &["status", "--porcelain"]).await?;
    let mut files: Vec<String> = status
        .lines()
        .filter_map(|l| l.split_whitespace().last().map(str::to_string))
        .collect();
    files.sort();
    files.dedup();
    Ok(GitDiff { files, raw })
}

/// Merge `ade/<slug>` into the repo checkout with `--no-ff`, then remove
/// the worktree. Fails cleanly on a dirty repo or conflicts so the user can
/// resolve by hand; nothing is deleted in that case.
pub async fn merge_winner(repo: &Path, slug: &str) -> Result<String, WorktreeError> {
    let dirty = run_git(repo, &["status", "--porcelain"]).await?;
    if !dirty.trim().is_empty() {
        return Err(WorktreeError::Git(
            "repo checkout has uncommitted changes — commit or stash first".into(),
        ));
    }
    let branch = branch_name(slug);
    let out = run_git(
        repo,
        &["merge", "--no-ff", &branch, "-m", &format!("merge: {slug}")],
    )
    .await
    .map_err(|e| {
        WorktreeError::Git(format!(
            "merge conflict in {branch} — resolve in the repo, then remove the worktree by hand: {e}"
        ))
    })?;
    remove(repo, slug).await?;
    Ok(out.trim().to_string())
}
/// `git worktree prune` plus deletion of merged `ade/*` orphan branches
/// whose worktree directory is gone.
pub async fn prune(repo: &Path) -> Result<(), WorktreeError> {
    run_git(repo, &["worktree", "prune"]).await?;
    let infos = list(repo).await.unwrap_or_default();
    let live_branches: Vec<&str> = infos.iter().filter_map(|i| i.branch.as_deref()).collect();
    let merged = run_git(repo, &["branch", "--merged", "HEAD", "--list", "ade/*"]).await?;
    for line in merged.lines() {
        let b = line.trim().trim_start_matches("* ").trim();
        if b.is_empty() || live_branches.contains(&b) {
            continue;
        }
        let slug = b.strip_prefix(BRANCH_PREFIX).unwrap_or(b);
        if !worktree_path(repo, slug).exists() {
            let _ = run_git(repo, &["branch", "-d", b]).await;
        }
    }
    Ok(())
}

async fn managed_count(repo: &Path) -> Result<usize, WorktreeError> {
    Ok(list(repo)
        .await
        .unwrap_or_default()
        .iter()
        .filter(|i| is_worktree_path(&i.path))
        .count())
}

async fn is_registered(repo: &Path, path: &Path) -> bool {
    list(repo)
        .await
        .unwrap_or_default()
        .iter()
        .any(|i| i.path == path)
}

async fn branch_checked_out(repo: &Path, slug: &str) -> Result<bool, WorktreeError> {
    let want = format!("refs/heads/{}", branch_name(slug));
    Ok(list(repo).await?.iter().any(|i| {
        i.branch
            .as_deref()
            .is_some_and(|b| b == want || b == branch_name(slug))
    }))
}

fn parse_porcelain(out: &str) -> Result<Vec<WorktreeInfo>, WorktreeError> {
    let mut infos = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    let mut detached = false;
    let mut flush =
        |path: &mut Option<PathBuf>, branch: &mut Option<String>, detached: &mut bool| {
            if let Some(p) = path.take() {
                infos.push(WorktreeInfo {
                    path: p,
                    branch: branch.take(),
                    detached: *detached,
                });
            }
            *detached = false;
        };
    for line in out.lines() {
        if line.is_empty() {
            flush(&mut path, &mut branch, &mut detached);
        } else if let Some(p) = line.strip_prefix("worktree ") {
            flush(&mut path, &mut branch, &mut detached);
            path = Some(PathBuf::from(p));
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
        } else if line == "detached" || line == "bare" {
            detached = true;
        }
    }
    flush(&mut path, &mut branch, &mut detached);
    Ok(infos)
}

fn copy_worktreeinclude(repo: &Path, dest: &Path) {
    let Ok(raw) = std::fs::read_to_string(repo.join(WORKTREE_INCLUDE_FILE)) else {
        return;
    };
    for entry in raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        // Stay inside the repo: reject absolute paths and `..`.
        let rel = Path::new(entry);
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| c == std::path::Component::ParentDir)
        {
            continue;
        }
        let src = repo.join(rel);
        let dst = dest.join(rel);
        if src.is_file() {
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(&src, &dst);
        } else if src.is_dir() {
            copy_dir_recursive(&src, &dst);
        }
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    let _ = std::fs::create_dir_all(dst);
    let Ok(entries) = std::fs::read_dir(src) else {
        return;
    };
    for e in entries.flatten() {
        let from = e.path();
        let to = dst.join(e.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to);
        } else if from.is_file() {
            let _ = std::fs::copy(&from, &to);
        }
    }
}

async fn run_git(repo: &Path, args: &[&str]) -> Result<String, WorktreeError> {
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .await
        .map_err(|e| WorktreeError::Io(e.to_string()))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(WorktreeError::Git(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_basic() {
        assert_eq!(normalize_slug("Feat Auth!"), "feat-auth");
        assert_eq!(normalize_slug("  --fix--api-- "), "fix-api");
        assert_eq!(normalize_slug(""), "");
        assert_eq!(normalize_slug("a"), "a");
    }

    #[test]
    fn record_paths() {
        let repo = Path::new("/repo");
        let r = WorktreeRecord::new(repo, "Feat Auth 3f2a").unwrap();
        assert_eq!(r.slug, "feat-auth-3f2a");
        assert_eq!(r.branch, "ade/feat-auth-3f2a");
        assert_eq!(r.path, PathBuf::from("/repo/.ade-worktrees/feat-auth-3f2a"));
    }

    #[test]
    fn rejects_empty_slug() {
        assert!(WorktreeRecord::new(Path::new("/r"), "!!!").is_none());
    }

    #[test]
    fn detects_worktree_path() {
        assert!(is_worktree_path(Path::new("/repo/.ade-worktrees/feat-x")));
        assert!(!is_worktree_path(Path::new("/repo/src/main.rs")));
    }
}
