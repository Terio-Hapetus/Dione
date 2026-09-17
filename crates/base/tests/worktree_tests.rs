//! Worktree git-ops tests against real temporary repos. No network.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base::worktree;

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
    let dir = std::env::temp_dir().join(format!("dione-wt-test-{n}-{c}-{}", std::process::id()));
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
        .args([
            "worktree",
            "remove",
            "--force",
            "--force",
            ".dione-worktrees",
        ])
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
    assert_eq!(r.branch, "dione/feat-auth");
    assert!(r.path.join("f.txt").exists());

    let infos = worktree::list(&repo).await.unwrap();
    assert!(
        infos
            .iter()
            .any(|i| i.path == r.path && i.branch.as_deref() == Some("dione/feat-auth"))
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
    sh(&repo, &["merge", "-q", "--ff-only", "dione/feat-gone"]);
    worktree::prune(&repo).await.unwrap();
    let branches = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["branch", "--list", "dione/*"])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&branches.stdout).contains("dione/feat-gone"),
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
        .args(["branch", "--list", "dione/*"])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&branches.stdout).contains("dione/feat-win"),
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

#[test]
fn split_hunks_groups_headers_and_bodies() {
    let patch = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\n-b\n+c\n@@ -9,1 +9,1 @@\n-x\n+y\n";
    let h = worktree::split_hunks(patch);
    assert_eq!(h.len(), 2);
    assert_eq!(h[0].header, "@@ -1,2 +1,2 @@");
    assert_eq!(h[0].lines, vec![" a", "-b", "+c"]);
    assert_eq!(h[1].header, "@@ -9,1 +9,1 @@");
    assert_eq!(h[1].lines, vec!["-x", "+y"]);
    assert!(worktree::split_hunks("no hunks here\n").is_empty());
}

/// Numbered file with 3 far-apart changes → 3 hunks, pristine again.
fn three_hunk_repo() -> (PathBuf, Vec<worktree::Hunk>) {
    let repo = init_repo();
    let body: String = (1..=30).map(|i| format!("L{i}\n")).collect();
    std::fs::write(repo.join("f.txt"), &body).unwrap();
    sh(&repo, &["add", "."]);
    sh(&repo, &["commit", "-qm", "numbered"]);
    let changed = body
        .replace("L5\n", "L5*\n")
        .replace("L15\n", "L15*\n")
        .replace("L25\n", "L25*\n");
    std::fs::write(repo.join("f.txt"), &changed).unwrap();
    let patch = git_out(&repo, &["diff", "--", "f.txt"]);
    let hunks = worktree::split_hunks(&patch);
    assert_eq!(hunks.len(), 3);
    sh(&repo, &["checkout", "--", "f.txt"]);
    (repo, hunks)
}

#[tokio::test]
async fn cherry_pick_applies_hunk_subset() {
    let (repo, hunks) = three_hunk_repo();
    worktree::apply_hunks(&repo, "f.txt", &[hunks[0].clone(), hunks[2].clone()])
        .await
        .unwrap();
    let got = std::fs::read_to_string(repo.join("f.txt")).unwrap();
    assert!(got.contains("L5*\n"), "hunk 0 applied");
    assert!(got.contains("L25*\n"), "hunk 2 applied");
    assert!(
        got.contains("L15\n") && !got.contains("L15*\n"),
        "hunk 1 skipped"
    );
    cleanup(&repo);
}

#[tokio::test]
async fn cherry_pick_conflict_fails_without_touching_file() {
    let (repo, hunks) = three_hunk_repo();
    worktree::apply_hunks(&repo, "f.txt", &hunks).await.unwrap();
    // Same hunk again: context no longer matches → clean error.
    let err = worktree::apply_hunks(&repo, "f.txt", &hunks[0..1])
        .await
        .unwrap_err();
    assert!(matches!(err, worktree::WorktreeError::Git(_)));
    let got = std::fs::read_to_string(repo.join("f.txt")).unwrap();
    assert!(got.contains("L5*\n"), "first apply kept");
    cleanup(&repo);
}

#[tokio::test]
async fn cherry_pick_rejects_empty_selection_and_bad_input() {
    let repo = init_repo();
    // Empty selection is a silent no-op (no git involved).
    worktree::apply_hunks(&repo, "f.txt", &[]).await.unwrap();
    // Bad file names and headers fail before touching git.
    let h = worktree::Hunk {
        header: "@@ -1,1 +1,1 @@".into(),
        lines: vec!["-hi".into(), "+ho".into()],
    };
    assert!(
        worktree::apply_hunks(&repo, "", std::slice::from_ref(&h))
            .await
            .is_err()
    );
    let bad = worktree::Hunk {
        header: "not a header".into(),
        lines: vec![],
    };
    assert!(worktree::apply_hunks(&repo, "f.txt", &[bad]).await.is_err());
    // Traversal, absolute, multi-file labels, and sentinels never run git.
    for evil in [
        "../evil.txt",
        "/abs.txt",
        "a.rs, b.rs",
        "(working tree)",
        "(unknown)",
    ] {
        assert!(
            worktree::apply_hunks(&repo, evil, std::slice::from_ref(&h))
                .await
                .is_err(),
            "{evil} must be rejected"
        );
    }
    cleanup(&repo);
}

#[tokio::test]
async fn create_branch_here_pins_agent_branch() {
    let repo = init_repo();
    let r = worktree::create(&repo, "feat-br").await.unwrap();
    std::fs::write(r.path.join("br.txt"), "b\n").unwrap();
    sh(&r.path, &["add", "."]);
    sh(&r.path, &["commit", "-qm", "br work"]);

    let name = worktree::create_branch_here(&repo, "feat-br", "local/feat-br")
        .await
        .unwrap();
    assert_eq!(name, "local/feat-br");
    assert_eq!(
        git_out(&repo, &["rev-parse", "local/feat-br"]),
        git_out(&repo, &["rev-parse", "dione/feat-br"])
    );
    // Collision, bad names, and missing source branches fail cleanly.
    assert!(
        worktree::create_branch_here(&repo, "feat-br", "local/feat-br")
            .await
            .is_err()
    );
    assert!(
        worktree::create_branch_here(&repo, "feat-br", "  ")
            .await
            .is_err()
    );
    assert!(
        worktree::create_branch_here(&repo, "feat-br", "bad..name")
            .await
            .is_err()
    );
    assert!(
        worktree::create_branch_here(&repo, "nope", "local/x")
            .await
            .is_err()
    );

    worktree::remove(&repo, "feat-br").await.unwrap();
    sh(&repo, &["branch", "-D", "local/feat-br"]);
    cleanup(&repo);
}

#[tokio::test]
async fn hand_off_merges_but_keeps_worktree() {
    let repo = init_repo();
    let r = worktree::create(&repo, "feat-ho").await.unwrap();
    std::fs::write(r.path.join("ho.txt"), "h\n").unwrap();
    sh(&r.path, &["add", "."]);
    sh(&r.path, &["commit", "-qm", "ho work"]);
    // Diverge main (also sweeps the worktree gitlink into the index,
    // like the merge_winner test, so the dirty guard passes).
    std::fs::write(repo.join("main.txt"), "main\n").unwrap();
    sh(&repo, &["add", "."]);
    sh(&repo, &["commit", "-qm", "main work"]);

    let summary = worktree::hand_off_to_local(&repo, "feat-ho").await.unwrap();
    assert!(summary.contains("Merge"), "unexpected: {summary}");
    assert_eq!(std::fs::read_to_string(repo.join("ho.txt")).unwrap(), "h\n");
    // Worktree AND branch survive (unlike merge_winner).
    assert!(r.path.exists());
    assert!(git_out(&repo, &["branch", "--list", "dione/feat-ho"]).contains("dione/feat-ho"));

    worktree::remove(&repo, "feat-ho").await.unwrap();
    cleanup(&repo);
}

#[tokio::test]
async fn cherry_pick_multi_file_raw_applies_per_file() {
    use base::split_files;

    // Two files changed at once: the combined raw must split per file,
    // and each file's hunks must apply to that file alone.
    let repo = init_repo();
    for (name, body) in [("a.txt", "A1\nA2\nA3\n"), ("b.txt", "B1\nB2\nB3\n")] {
        std::fs::write(repo.join(name), body).unwrap();
    }
    sh(&repo, &["add", "."]);
    sh(&repo, &["commit", "-qm", "two files"]);
    std::fs::write(repo.join("a.txt"), "A1\nA2*\nA3\n").unwrap();
    std::fs::write(repo.join("b.txt"), "B1\nB2*\nB3\n").unwrap();
    let raw = git_out(&repo, &["diff", "--", "a.txt", "b.txt"]);

    let parts = split_files(&raw);
    assert_eq!(parts.len(), 2);
    for (file, section) in &parts {
        let f = file.as_deref().unwrap();
        let hunks = worktree::split_hunks(section);
        assert_eq!(hunks.len(), 1, "one hunk for {f}");
        // Restore pristine, apply only this file's hunk to this file.
        sh(&repo, &["checkout", "--", f]);
        worktree::apply_hunks(&repo, f, &hunks).await.unwrap();
    }
    assert!(
        std::fs::read_to_string(repo.join("a.txt"))
            .unwrap()
            .contains("A2*\n")
    );
    assert!(
        std::fs::read_to_string(repo.join("b.txt"))
            .unwrap()
            .contains("B2*\n")
    );
    cleanup(&repo);
}
