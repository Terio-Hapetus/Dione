//! Live Cloud Hypervisor loop (Lab 2 + Lab 3):
//! pull -> boot -> wait_ssh -> mount 2-way -> stop.
//! Runs ONLY with `ADE_LIVE_VM=1` (needs KVM, cloud-hypervisor,
//! virtiofsd, genisoimage, ADE_VM_KERNEL(_URL)/ADE_VM_IMAGE(_URL)).
//! Without it, SKIP-passes so default CI stays green.

use std::process::Command;
use std::time::Duration;

use ade_vm::{
    CloudHypervisorBackend, EphemeralKey, VmConfig, VmManager, VmTimeouts, default_cache_dir,
};

fn live() -> bool {
    std::env::var("ADE_LIVE_VM").as_deref() == Ok("1")
}

fn ssh(info: &ade_vm::SshInfo, key: &std::path::Path, remote: &str) -> bool {
    Command::new("ssh")
        .args([
            "-i",
            &key.to_string_lossy(),
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-p",
            &info.port.to_string(),
            &format!("{}@{}", info.user, info.host),
            remote,
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn ch_boot_mount_stop() {
    if !live() {
        eprintln!("SKIP: set ADE_LIVE_VM=1 on a KVM machine (see Lab 2)");
        return;
    }
    let repo = std::env::temp_dir().join(format!("ade-live-ch-{}", std::process::id()));
    std::fs::create_dir_all(&repo).unwrap();
    let key = EphemeralKey::generate().expect("ssh-keygen");
    let cfg = VmConfig {
        mount_repo: repo.clone(),
        ssh_pubkey: key.pubkey_openssh().to_string(),
        ..VmConfig::default()
    };
    let timeouts = VmTimeouts {
        ssh: Duration::from_secs(120),
        mount: Duration::from_secs(60),
        ..VmTimeouts::default()
    };
    let vm_dir = default_cache_dir().join("live-test");
    let mut mgr = VmManager::with_timeouts(CloudHypervisorBackend::new(vm_dir), timeouts);

    // Mount check = Lab 3, both directions, over raw ssh.
    // (Runs after ensure_ready: the closure would borrow mgr twice.
    // Manager retry logic itself is unit-tested in manager.rs.)
    let probe = format!("ade-probe-{}", std::process::id());
    let handle = mgr
        .ensure_ready(&repo, &cfg, || Ok(()))
        .expect("CH ensure_ready failed");
    eprintln!("ready: {}", handle.id);
    let info = mgr.ssh_info(&repo).expect("ssh_info failed");
    // Guest -> host.
    assert!(ssh(
        &info,
        key.priv_path(),
        &format!("touch /workspace/{probe} && echo ok"),
    ));
    assert!(repo.join(&probe).exists(), "mount invisible on host");
    std::fs::remove_file(repo.join(&probe)).unwrap();
    // Host -> guest.
    std::fs::write(repo.join(&probe), b"hi").unwrap();
    assert!(ssh(
        &info,
        key.priv_path(),
        &format!("test -f /workspace/{probe}")
    ));
    std::fs::remove_file(repo.join(&probe)).unwrap();
    mgr.stop(&repo).expect("stop failed");
    let _ = std::fs::remove_dir_all(&repo);
}
