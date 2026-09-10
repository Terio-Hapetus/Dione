//! Full sbx loop through VmManager + MicroVm provider:
//! boot → exec → 2-way mount check → stop. Runs ONLY with
//! `ADE_LIVE_SBX=1` (needs sbx login + KVM); otherwise SKIP-pass.

use std::time::Duration;

use ade_vm::{EphemeralKey, ExternalSbxBackend, VmConfig, VmManager, VmTimeouts};
use ade_workspace::{MicroVm, PathMapping, SshTarget, WorkspaceProvider};

fn live() -> bool {
    std::env::var("ADE_LIVE_SBX").as_deref() == Ok("1")
}

#[test]
fn sbx_manager_mount_loop() {
    if !live() {
        eprintln!("SKIP: set ADE_LIVE_SBX=1 to run against real sbx");
        return;
    }
    let repo = std::env::temp_dir().join(format!("ade-live-ws-{}", std::process::id()));
    std::fs::create_dir_all(&repo).unwrap();

    let key = EphemeralKey::generate().expect("ssh-keygen");
    let cfg = VmConfig {
        mount_repo: repo.clone(),
        ssh_pubkey: key.pubkey_openssh().to_string(),
        ..VmConfig::default()
    };
    // NOTE: sbx images rarely read VmConfig.ssh_pubkey; if this run shows
    // auth failures, wire the key via `sbx secret`/setup before boot and
    // record the working step in sbx.rs docs.
    let timeouts = VmTimeouts {
        ssh: Duration::from_secs(120),
        mount: Duration::from_secs(60),
        ..VmTimeouts::default()
    };
    let mut mgr = VmManager::with_timeouts(ExternalSbxBackend::new(), timeouts);
    mgr.ensure_ready(&repo, &cfg, || Ok(()))
        .expect("sbx ensure_ready failed");

    let info = mgr.ssh_info(&repo).expect("ssh_info failed");
    let target = SshTarget {
        host: info.host,
        port: info.port,
        user: info.user,
        key_path: key.priv_path().to_path_buf(),
    };
    // sbx mounts absolute host paths: guest path == host path.
    let mut vm = MicroVm::new(target, PathMapping::Identity);

    // Guest → host: file created inside appears on the host.
    let probe = repo.join("hello-from-vm");
    vm.exec(&["touch", &probe.to_string_lossy()], &repo)
        .expect("guest touch failed");
    assert!(probe.exists(), "mount not visible on host");
    std::fs::remove_file(&probe).unwrap();

    // Host → guest: file created on host is visible inside.
    let back = repo.join("hello-from-host");
    std::fs::write(&back, b"hi").unwrap();
    let out = vm
        .exec(&["test", "-f", &back.to_string_lossy()], &repo)
        .expect("guest test failed");
    assert!(out.success(), "mount not visible in guest");

    mgr.stop(&repo).expect("stop failed");
    let _ = std::fs::remove_dir_all(&repo);
}
