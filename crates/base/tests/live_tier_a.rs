//! Tier A live check (M2 exit gate): real `opencode serve` + real git worktrees,
//! no LLM prompt. Run with:
//! `cargo test -p base --features integration-tests --test live_tier_a`
//! Tier B (real prompt, spends quota) is opt-in via `DIONE_LIVE_PROMPT=1`
//! and stays a manual step (see docs/STATUS.md).

#![cfg(feature = "integration-tests")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base::runtime::{self, Command};
use base::{AppConfig, ConnState};

fn git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dione-tier-a-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-b", "main"]);
    git(
        &dir,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    // opencode serve expects a project dir; a marker file is enough.
    // NOTE: `.dione-worktrees/` must be git-ignored, otherwise the merge
    // dirty-guard (correctly) refuses to merge — same rule as M2e.
    std::fs::write(dir.join("README.md"), "# tier-a\n").unwrap();
    std::fs::write(dir.join(".gitignore"), ".dione-worktrees/\n").unwrap();
    git(
        &dir,
        &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"],
    );
    git(
        &dir,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "readme",
        ],
    );
    dir
}

fn wait_for(desc: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < timeout, "timeout waiting for {desc}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn tier_a_serve_plus_fleet_no_prompt() {
    let repo = init_repo();
    let config = AppConfig {
        project_dir: repo.clone(),
        opencode_binary: "opencode".to_string(),
        poll_interval_ms: 300,
        busy_poll_interval_ms: 200,
    };
    let rt = runtime::spawn(config);

    // 1. Server boots and runtime connects.
    wait_for("connected", Duration::from_secs(40), || {
        matches!(rt.snapshot().conn, ConnState::Connected)
    });

    // 2. Two worktrees via runtime (mirrors `+ wt` in the UI).
    rt.send(Command::CreateWorktree {
        slug: "tier-a1".into(),
    });
    rt.send(Command::CreateWorktree {
        slug: "tier-a2".into(),
    });
    wait_for("2 worktrees with sessions", Duration::from_secs(40), || {
        let s = rt.snapshot();
        s.worktrees.len() == 2 && s.worktrees.values().all(|r| r.session_id.is_some())
    });
    assert!(
        rt.snapshot().errors.is_empty(),
        "errors: {:?}",
        rt.snapshot().errors
    );

    // 3. Diff fetch across sessions (REST reconcile path).
    rt.send(Command::FetchAllDiffs);
    std::thread::sleep(Duration::from_secs(2));

    // 4. Real change in tier-a1 → merge winner live.
    let wt1 = repo.join(".dione-worktrees").join("tier-a1");
    std::fs::write(wt1.join("hello.txt"), "from tier-a1\n").unwrap();
    git(
        &wt1,
        &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"],
    );
    git(
        &wt1,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "tier-a1 change",
        ],
    );
    rt.send(Command::MergeWorktree {
        slug: "tier-a1".into(),
    });
    wait_for("tier-a1 merged away", Duration::from_secs(30), || {
        !rt.snapshot().worktrees.contains_key("tier-a1")
    });
    assert!(
        repo.join("hello.txt").exists(),
        "merge result visible on host checkout"
    );

    // 5. Remove the loser.
    rt.send(Command::RemoveWorktree {
        slug: "tier-a2".into(),
    });
    wait_for("tier-a2 removed", Duration::from_secs(30), || {
        !rt.snapshot().worktrees.contains_key("tier-a2")
    });

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn tier_b_real_prompt_opt_in_only() {
    if std::env::var("DIONE_LIVE_PROMPT").is_err() {
        eprintln!("skipping Tier B: set DIONE_LIVE_PROMPT=1 to spend quota on a real prompt");
        return;
    }
    let repo = init_repo();
    let config = AppConfig {
        project_dir: repo.clone(),
        opencode_binary: "opencode".to_string(),
        poll_interval_ms: 500,
        busy_poll_interval_ms: 300,
    };
    let rt = runtime::spawn(config);
    wait_for("connected", Duration::from_secs(40), || {
        matches!(rt.snapshot().conn, ConnState::Connected)
    });
    rt.send(Command::CreateWorktree {
        slug: "tier-b".into(),
    });
    wait_for("tier-b session", Duration::from_secs(40), || {
        rt.snapshot()
            .worktrees
            .get("tier-b")
            .is_some_and(|r| r.session_id.is_some())
    });
    rt.send(Command::Prompt {
        text: "Reply with exactly: TIER-B-OK. Do not edit any files.".into(),
    });
    wait_for("agent answered", Duration::from_secs(180), || {
        rt.snapshot().active_session.as_ref().is_some_and(|sid| {
            rt.snapshot()
                .messages
                .get(sid)
                .is_some_and(|m| !m.is_empty())
        })
    });
    let _ = std::fs::remove_dir_all(&repo);
}
