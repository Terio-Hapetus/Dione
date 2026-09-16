//! Live podman loop (replaces Lab 2 + Lab 3 for containers):
//! pull -> run -> exec touch 2-way -> stop.
//! Runs ONLY with `DIONE_LIVE_PODMAN=1` (needs podman + network for pull).
//! Without it, SKIP-passes so default CI stays green.

use std::path::Path;

use workspace::{
    ContainerManager, ContainerSpec, ContainerState, PodmanProvider, WorkspaceProvider,
};

fn live() -> bool {
    std::env::var("DIONE_LIVE_PODMAN").as_deref() == Ok("1")
}

#[test]
fn podman_run_exec_stop() {
    if !live() {
        eprintln!("SKIP: set DIONE_LIVE_PODMAN=1 on a podman machine");
        return;
    }
    if !workspace::probe_podman() {
        eprintln!("SKIP: podman binary not found");
        return;
    }
    let root = std::env::temp_dir().join(format!("dione-live-podman-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let spec = ContainerSpec::for_workspace(&root);
    let mgr = ContainerManager::new();
    assert_eq!(mgr.ensure_running(&spec), ContainerState::Running);
    let probe = "hello-from-ctr";
    let mut p = PodmanProvider::new(&spec.name, &root);
    let out = p
        .exec(
            &["sh", "-c", &format!("touch /workspace/{probe} && echo ok")],
            Path::new(&root),
        )
        .expect("exec in container");
    assert!(out.success(), "stdout={} stderr={}", out.stdout, out.stderr);
    assert!(
        root.join(probe).exists(),
        "bind mount must show the file on host"
    );
    assert!(mgr.stop(&spec.name).expect("stop"));
    let _ = std::fs::remove_dir_all(&root);
}

/// Extended live loop (Lab 2/3 hardening): ensure is idempotent, an
/// interactive `exec -it` shell works under pty (Terminal-tab path),
/// and stop cleans up. Same gate as above: real podman only.
#[test]
fn podman_ensure_idempotent_shell_stop() {
    if !live() {
        eprintln!("SKIP: set DIONE_LIVE_PODMAN=1 on a podman machine");
        return;
    }
    if !workspace::probe_podman() {
        eprintln!("SKIP: podman binary not found");
        return;
    }
    let root = std::env::temp_dir().join(format!("dione-live-podman-x-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let spec = ContainerSpec::for_workspace(&root);
    let mgr = ContainerManager::new();
    // Idempotent ensure: twice → Running, still exactly one container.
    assert_eq!(mgr.ensure_running(&spec), ContainerState::Running);
    assert_eq!(mgr.ensure_running(&spec), ContainerState::Running);
    assert_eq!(count_named(&spec.name), 1, "ensure must not duplicate");
    // Interactive shell (same `exec -it` path the Terminal tab uses).
    let mut p = PodmanProvider::new(&spec.name, &root);
    let mut sh = p.shell(Path::new(&root)).expect("container shell");
    sh.write_bytes(b"echo live-shell-ok\n").unwrap();
    let mut acc = Vec::new();
    for _ in 0..200 {
        acc.extend(sh.read_available());
        if String::from_utf8_lossy(&acc).contains("live-shell-ok") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        String::from_utf8_lossy(&acc).contains("live-shell-ok"),
        "shell roundtrip failed"
    );
    sh.kill().unwrap();
    // Clean stop: first true, second false, then gone.
    assert!(mgr.stop(&spec.name).expect("stop"));
    assert!(!mgr.stop(&spec.name).expect("second stop"));
    assert_eq!(count_named(&spec.name), 0);
    let _ = std::fs::remove_dir_all(&root);
}

/// How many containers exist with exactly this name (`podman ps -a`).
fn count_named(name: &str) -> usize {
    let out = std::process::Command::new("podman")
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("name=^{name}$"),
            "--format",
            "{{.Names}}",
        ])
        .output()
        .expect("podman ps");
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.trim() == name)
        .count()
}
