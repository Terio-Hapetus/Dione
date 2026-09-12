//! M6 W2: VmManager background thread — the UI thread never blocks.
//!
//! Backend is chosen once by `probe_kvm`: CloudHypervisor on KVM machines,
//! Mock elsewhere (instant Ready, loopback ports). The UI sends [`VmCmd`]
//! and drains [`VmReport`]s in its 160ms loop; `set_vm_state` feeds the
//! Fleet badge. Ephemeral keys live here as long as their VM does.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use ade_vm::{
    CloudHypervisorBackend, EphemeralKey, MockBackend, VmConfig, VmManager, VmState,
    default_cache_dir, probe_kvm,
};

/// UI → VM thread.
#[derive(Debug)]
pub enum VmCmd {
    Ensure { slug: String, path: PathBuf },
    Stop { slug: String, path: PathBuf },
}

/// VM thread → UI.
#[derive(Debug, Clone)]
pub struct VmReport {
    pub slug: String,
    pub state: VmState,
}

enum Backend {
    Mock(VmManager<MockBackend>),
    Ch(VmManager<CloudHypervisorBackend>),
}

/// Workspace root for a checkout: parent of `.ade-worktrees` when nested,
/// else the path itself. VMs converge to 1-per-workspace (M7); until then
/// each worktree maps to its repo this way.
fn workspace_root(path: &Path) -> PathBuf {
    let mut cur: Option<&Path> = Some(path);
    while let Some(p) = cur {
        if p.file_name().is_some_and(|n| n == ".ade-worktrees")
            && let Some(parent) = p.parent()
        {
            return parent.to_path_buf();
        }
        cur = p.parent();
    }
    path.to_path_buf()
}

/// Handle the UI owns: send commands, drain reports (non-blocking).
pub struct VmThread {
    tx: Sender<VmCmd>,
    rx: Receiver<VmReport>,
}

impl VmThread {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = channel::<VmCmd>();
        let (rep_tx, rep_rx) = channel::<VmReport>();
        std::thread::spawn(move || vm_loop(cmd_rx, rep_tx));
        Self {
            tx: cmd_tx,
            rx: rep_rx,
        }
    }

    pub fn ensure(&self, slug: String, path: PathBuf) {
        let _ = self.tx.send(VmCmd::Ensure { slug, path });
    }

    pub fn stop(&self, slug: String, path: PathBuf) {
        let _ = self.tx.send(VmCmd::Stop { slug, path });
    }

    pub fn drain(&self) -> Vec<VmReport> {
        self.rx.try_iter().collect()
    }
}

fn vm_loop(cmd_rx: Receiver<VmCmd>, rep_tx: Sender<VmReport>) {
    let mut backend = if probe_kvm() {
        Backend::Ch(VmManager::new(CloudHypervisorBackend::new(
            default_cache_dir(),
        )))
    } else {
        Backend::Mock(VmManager::new(MockBackend::new()))
    };
    // Workspace root → key (wipe on stop) + slug (for reports).
    let mut keys: BTreeMap<PathBuf, EphemeralKey> = BTreeMap::new();
    let mut slugs: BTreeMap<PathBuf, String> = BTreeMap::new();

    let report = |slug: &str, state: VmState| {
        let _ = rep_tx.send(VmReport {
            slug: slug.to_string(),
            state,
        });
    };

    for cmd in cmd_rx {
        match cmd {
            VmCmd::Ensure { slug, path } => {
                let root = workspace_root(&path);
                slugs.insert(root.clone(), slug.clone());
                // Optimistic badge; the real state follows ensure_ready.
                report(&slug, VmState::Booting);
                let key = match EphemeralKey::generate() {
                    Ok(k) => k,
                    Err(e) => {
                        report(&slug, VmState::Error(format!("keygen failed: {e:#}")));
                        continue;
                    }
                };
                let cfg = VmConfig {
                    mount_repo: root.clone(),
                    ssh_pubkey: key.pubkey_openssh().to_string(),
                    ..VmConfig::default()
                };
                keys.insert(root.clone(), key);
                // Mount check is a live-VM concern; the Mock path is
                // trivially mounted. (M7: provider touch both directions.)
                let check = || Ok(());
                let st = match &mut backend {
                    Backend::Mock(m) => m.ensure_ready(&root, &cfg, check),
                    Backend::Ch(m) => m.ensure_ready(&root, &cfg, check),
                };
                match st {
                    Ok(_) => {
                        let state = match &mut backend {
                            Backend::Mock(m) => m.state_of_workspace(&root),
                            Backend::Ch(m) => m.state_of_workspace(&root),
                        };
                        report(&slug, state);
                    }
                    Err(e) => report(&slug, VmState::Error(format!("{e:#}"))),
                }
            }
            VmCmd::Stop { slug, path } => {
                let root = workspace_root(&path);
                let stopped = match &mut backend {
                    Backend::Mock(m) => m.stop(&root),
                    Backend::Ch(m) => m.stop(&root),
                };
                keys.remove(&root);
                slugs.remove(&root);
                match stopped {
                    Ok(true) => report(&slug, VmState::Stopped),
                    Ok(false) => report(&slug, VmState::Missing),
                    Err(e) => report(&slug, VmState::Error(format!("{e:#}"))),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_root_collapses_worktrees() {
        assert_eq!(
            workspace_root(Path::new("/repo/.ade-worktrees/feat-a")),
            PathBuf::from("/repo")
        );
        assert_eq!(
            workspace_root(Path::new("/repo/.ade-worktrees")),
            PathBuf::from("/repo")
        );
        assert_eq!(workspace_root(Path::new("/repo")), PathBuf::from("/repo"));
    }

    #[test]
    fn ensure_then_stop_roundtrips_without_kvm() {
        let vm = VmThread::spawn();
        vm.ensure("wt-a".into(), PathBuf::from("/tmp/ws-a"));
        // The optimistic Booting report arrives regardless of backend.
        let mut got = Vec::new();
        for _ in 0..100 {
            got.extend(vm.drain());
            if !got.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(got.iter().any(|r| r.slug == "wt-a"));
        // Full Ready/Stop flow only on Mock (no KVM); a KVM machine
        // would boot a real guest here (live-gated, not in unit tests).
        if probe_kvm() {
            return;
        }
        for _ in 0..100 {
            got.extend(vm.drain());
            if got.iter().any(|r| matches!(r.state, VmState::Ready)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(got.iter().any(|r| matches!(r.state, VmState::Ready)));
        vm.stop("wt-a".into(), PathBuf::from("/tmp/ws-a"));
        let mut gone = false;
        for _ in 0..100 {
            if vm
                .drain()
                .iter()
                .any(|r| matches!(r.state, VmState::Stopped | VmState::Missing))
            {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(gone);
    }
}
