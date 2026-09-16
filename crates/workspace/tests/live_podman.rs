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
