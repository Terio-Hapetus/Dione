//! Live sbx loop: boot → wait_ssh → stop against real `sbx`.
//! Runs ONLY with `ADE_LIVE_SBX=1` (needs sbx login + KVM).
//! Without it, prints SKIP and passes so default CI stays green.

use std::time::{Duration, Instant};

use ade_vm::{ExternalSbxBackend, VmBackend, VmConfig};

fn live() -> bool {
    std::env::var("ADE_LIVE_SBX").as_deref() == Ok("1")
}

#[test]
fn sbx_boot_ssh_stop() {
    if !live() {
        eprintln!("SKIP: set ADE_LIVE_SBX=1 to run against real sbx");
        return;
    }
    let dir = std::env::temp_dir().join(format!("ade-live-sbx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut b = ExternalSbxBackend::new();
    let cfg = VmConfig {
        mount_repo: dir.clone(),
        ..VmConfig::default()
    };
    let h = b.boot(&cfg).expect("sbx create failed");
    // Poll one-shot wait_ssh until the 60s live budget expires.
    let start = Instant::now();
    let ssh = loop {
        match b.wait_ssh(&h) {
            Ok(s) => break s,
            Err(e) if start.elapsed() < Duration::from_secs(60) => {
                eprintln!("ssh not ready: {e:#}; retrying…");
                std::thread::sleep(Duration::from_secs(2));
            }
            Err(e) => panic!("ssh never ready: {e:#}"),
        }
    };
    eprintln!("ssh ready: {}@{}:{}", ssh.user, ssh.host, ssh.port);
    // LIVE-VERIFY (update sbx.rs if wrong): user field, `ls` statuses.
    eprintln!("state: {:?}", b.state_of(&h));
    b.stop(&h).expect("sbx rm failed");
    let _ = std::fs::remove_dir_all(&dir);
}
