//! Worktree git-ops tests against real temporary repos. No network.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ade_core::worktree;

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn sh(repo: &Path, args: &[&str]) {
    let st = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("git must run");
    assert!(st.success(), "git {args:?} failed");
}

fn init_repo() -> PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let c = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("ade-wt-test-{n}-{c}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    sh(&dir, &["init", "-b", "main"]);
    sh(&dir, &["config", "user.email", "t@t"]);
    sh(&dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("f.txt"), "hi\n").unwrap();
    sh(&dir, &["add", "."]);
    sh(&dir, &["commit", "-qm", "init"]);
    dir
}

fn cleanup(dir: &Path) {
    // Worktrees must go before the main checkout dir can be removed.
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["worktree", "remove", "--force", "--force", ".ade-worktrees"])
        .output();
    let _ = std::fs::remove_dir_all(dir);
}

fn git_out(repo: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git must run");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[tokio::test]
async fn resolve_base_falls_back_to_head() {
    let repo = init_repo();
    assert_eq!(worktree::resolve_base(&repo).await, "HEAD");
    cleanup(&repo);
}

#[tokio::test]
async fn create_pins_origin_head_when_present() {
    let repo = init_repo();
    // Fake remote over a local bare repo (no network): push A, then move
    // local main to B without pushing — origin/HEAD stays at A.
    let remote = repo.join("remote.git");
    sh(&repo, &["init", "-q", "--bare", remote.to_str().unwrap()]);
    sh(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    sh(&repo, &["push", "-q", "origin", "main"]);
    sh(&repo, &["remote", "set-head", "origin", "main"]);
    std::fs::write(repo.join("b.txt"), "b\n").unwrap();
    sh(&repo, &["add", "."]);
    sh(&repo, &["commit", "-qm", "B"]);

    assert_eq!(worktree::resolve_base(&repo).await, "origin/HEAD");
    let r = worktree::create(&repo, "feat-base").await.unwrap();
    // New branch starts at origin/HEAD (A), not at local HEAD (B).
    let base_sha = git_out(&repo, &["rev-parse", "origin/HEAD"]);
    let head_sha = git_out(&repo, &["rev-parse", "HEAD"]);
    assert_ne!(base_sha, head_sha);
    assert_eq!(git_out(&r.path, &["rev-parse", "HEAD"]), base_sha);
    assert!(!r.path.join("b.txt").exists());

    worktree::remove(&repo, "feat-base").await.unwrap();
    sh(&repo, &["remote", "remove", "origin"]);
    cleanup(&repo);
}

#[tokio::test]
async fn create_adds_branch_and_dir() {
    let repo = init_repo();
    let r = worktree::create(&repo, "Feat Auth!").await.unwrap();
    assert_eq!(r.slug, "feat-auth");
    assert_eq!(r.branch, "ade/feat-auth");
    assert!(r.path.join("f.txt").exists());

    let infos = worktree::list(&repo).await.unwrap();
    assert!(
        infos
            .iter()
            .any(|i| i.path == r.path && i.branch.as_deref() == Some("ade/feat-auth"))
    );

    worktree::remove(&repo, "feat-auth").await.unwrap();
    assert!(!r.path.exists());
    cleanup(&repo);
}

#[tokio::test]
async fn duplicate_create_fails() {
    let repo = init_repo();
    worktree::create(&repo, "feat-x").await.unwrap();
    let err = worktree::create(&repo, "feat-x").await.unwrap_err();
    assert!(matches!(
        err,
        worktree::WorktreeError::AlreadyExists(_) | worktree::WorktreeError::Git(_)
    ));
    worktree::remove(&repo, "feat-x").await.unwrap();
    cleanup(&repo);
}

#[tokio::test]
async fn worktreeinclude_copies_listed_files() {
    let repo = init_repo();
    std::fs::write(repo.join(".env"), "K=V\n").unwrap();
    std::fs::write(repo.join(".worktreeinclude"), ".env\n# comment\n\n").unwrap();
    let r = worktree::create(&repo, "feat-env").await.unwrap();
    assert_eq!(
        std::fs::read_to_string(r.path.join(".env")).unwrap(),
        "K=V\n"
    );
    worktree::remove(&repo, "feat-env").await.unwrap();
    cleanup(&repo);
}

#[tokio::test]
async fn prune_drops_merged_orphan_branch() {
    let repo = init_repo();
    let r = worktree::create(&repo, "feat-gone").await.unwrap();
    // Simulate a crash: unregister without deleting the branch.
    sh(
        &repo,
        &["worktree", "remove", "--force", &r.path.to_string_lossy()],
    );
    sh(&repo, &["checkout", "-q", "main"]);
    sh(&repo, &["merge", "-q", "--ff-only", "ade/feat-gone"]);
    worktree::prune(&repo).await.unwrap();
    let branches = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["branch", "--list", "ade/*"])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&branches.stdout).contains("ade/feat-gone"),
        "orphan branch should be pruned"
    );
    cleanup(&repo);
}

#[tokio::test]
async fn merge_winner_brings_files_and_cleans_up() {
    let repo = init_repo();
    let r = worktree::create(&repo, "feat-win").await.unwrap();
    std::fs::write(r.path.join("win.txt"), "winner\n").unwrap();
    sh(&r.path, &["add", "."]);
    sh(&r.path, &["commit", "-qm", "win"]);
    // Diverge main so the merge is a real --no-ff merge commit.
    std::fs::write(repo.join("main.txt"), "main\n").unwrap();
    sh(&repo, &["add", "."]);
    sh(&repo, &["commit", "-qm", "main work"]);

    let summary = worktree::merge_winner(&repo, "feat-win").await.unwrap();
    assert!(
        summary.contains("Merge"),
        "unexpected merge output: {summary}"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("win.txt")).unwrap(),
        "winner\n"
    );
    assert!(!r.path.exists());
    let branches = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["branch", "--list", "ade/*"])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&branches.stdout).contains("ade/feat-win"),
        "branch should be gone after merge"
    );
    cleanup(&repo);
}

#[tokio::test]
async fn merge_winner_refuses_dirty_repo() {
    let repo = init_repo();
    let r = worktree::create(&repo, "feat-dirty").await.unwrap();
    std::fs::write(repo.join("uncommitted.txt"), "x\n").unwrap();
    let err = worktree::merge_winner(&repo, "feat-dirty")
        .await
        .unwrap_err();
    assert!(matches!(err, worktree::WorktreeError::Git(_)));
    // Nothing deleted on failure.
    assert!(r.path.exists());
    worktree::remove(&repo, "feat-dirty").await.unwrap();
    cleanup(&repo);
}

#[tokio::test]
async fn git_diff_sees_uncommitted_changes() {
    let repo = init_repo();
    let r = worktree::create(&repo, "feat-diff").await.unwrap();
    // Clean checkout: empty diff.
    let clean = worktree::git_diff(&r.path).await.unwrap();
    assert!(clean.is_empty());

    std::fs::write(r.path.join("f.txt"), "changed\n").unwrap();
    std::fs::write(r.path.join("new.txt"), "untracked\n").unwrap();
    let d = worktree::git_diff(&r.path).await.unwrap();
    assert!(!d.is_empty());
    assert!(d.files.contains(&"f.txt".to_string()));
    assert!(d.files.contains(&"new.txt".to_string()));
    assert!(d.raw.contains("changed"), "raw diff: {}", d.raw);
    // JSON shape fits the legacy `diffs` map.
    assert_eq!(d.to_json()["source"], "git");

    worktree::remove(&repo, "feat-diff").await.unwrap();
    cleanup(&repo);
}
