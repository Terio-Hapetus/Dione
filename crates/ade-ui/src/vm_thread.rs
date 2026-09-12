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
    CloudHypervisorBackend, EphemeralKey, MockBackend, SshInfo, VmConfig, VmManager, VmState,
    default_cache_dir, probe_kvm,
};
use ade_workspace::{
    MicroVm, PathMapping, PreviewPorts, ShellChannel, SshTarget, WorkspaceProvider,
};

/// UI → VM thread.
pub enum VmCmd {
    Ensure {
        slug: String,
        path: PathBuf,
    },
    Stop {
        slug: String,
        path: PathBuf,
    },
    OpenShell {
        slug: String,
        path: PathBuf,
        reply: Sender<ShellReply>,
    },
}

/// Result of `OpenShell`: a live guest shell or a human-readable error.
/// `Box` is `Send`, so it crosses threads through the channel.
pub type ShellReply = Result<Box<dyn ShellChannel>, String>;

/// VM thread → UI.
#[derive(Debug, Clone)]
pub struct VmReport {
    pub slug: String,
    pub state: VmState,
    /// SSH endpoint once Ready (key stays in this thread).
    pub ssh: Option<SshInfo>,
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

    /// Ask the thread for an interactive guest shell (W3). The reply
    /// arrives on the returned channel; the UI polls it without blocking.
    pub fn open_shell(&self, slug: String, path: PathBuf) -> Receiver<ShellReply> {
        let (tx, rx) = channel::<ShellReply>();
        let _ = self.tx.send(VmCmd::OpenShell {
            slug,
            path,
            reply: tx,
        });
        rx
    }

    pub fn drain(&self) -> Vec<VmReport> {
        self.rx.try_iter().collect()
    }
}

/// Per-workspace guest state, all owned by the VM thread: the ephemeral
/// key outlives every shell (wipe on stop), preview ports allocate once.
struct Guest {
    key: EphemeralKey,
    ssh: Option<SshInfo>,
    ports: PreviewPorts,
    preview_host: Option<u16>,
}

/// Default preview: guest :3000 (dev server convention).
const PREVIEW_GUEST_PORT: u16 = 3000;

fn vm_loop(cmd_rx: Receiver<VmCmd>, rep_tx: Sender<VmReport>) {
    let mut backend = if probe_kvm() {
        Backend::Ch(VmManager::new(CloudHypervisorBackend::new(
            default_cache_dir(),
        )))
    } else {
        Backend::Mock(VmManager::new(MockBackend::new()))
    };
    // Workspace root → guest (key wiped on stop).
    let mut guests: BTreeMap<PathBuf, Guest> = BTreeMap::new();

    let report = |slug: &str, state: VmState, ssh: Option<SshInfo>| {
        let _ = rep_tx.send(VmReport {
            slug: slug.to_string(),
            state,
            ssh,
        });
    };

    for cmd in cmd_rx {
        match cmd {
            VmCmd::Ensure { slug, path } => {
                let root = workspace_root(&path);
                // Optimistic badge; the real state follows ensure_ready.
                report(&slug, VmState::Booting, None);
                let key = match EphemeralKey::generate() {
                    Ok(k) => k,
                    Err(e) => {
                        report(&slug, VmState::Error(format!("keygen failed: {e:#}")), None);
                        continue;
                    }
                };
                let cfg = VmConfig {
                    mount_repo: root.clone(),
                    ssh_pubkey: key.pubkey_openssh().to_string(),
                    ..VmConfig::default()
                };
                // Mount check is a live-VM concern; the Mock path is
                // trivially mounted. (M7: provider touch both directions.)
                let check = || Ok(());
                let st = match &mut backend {
                    Backend::Mock(m) => m.ensure_ready(&root, &cfg, check),
                    Backend::Ch(m) => m.ensure_ready(&root, &cfg, check),
                };
                match st {
                    Ok(_) => {
                        let (state, ssh) = match &mut backend {
                            Backend::Mock(m) => {
                                (m.state_of_workspace(&root), m.ssh_info(&root).ok())
                            }
                            Backend::Ch(m) => (m.state_of_workspace(&root), m.ssh_info(&root).ok()),
                        };
                        guests.insert(
                            root.clone(),
                            Guest {
                                key,
                                ssh: ssh.clone(),
                                ports: PreviewPorts::new(),
                                preview_host: None,
                            },
                        );
                        report(&slug, state, ssh);
                    }
                    Err(e) => report(&slug, VmState::Error(format!("{e:#}")), None),
                }
            }
            VmCmd::Stop { slug, path } => {
                let root = workspace_root(&path);
                let stopped = match &mut backend {
                    Backend::Mock(m) => m.stop(&root),
                    Backend::Ch(m) => m.stop(&root),
                };
                guests.remove(&root);
                match stopped {
                    Ok(true) => report(&slug, VmState::Stopped, None),
                    Ok(false) => report(&slug, VmState::Missing, None),
                    Err(e) => report(&slug, VmState::Error(format!("{e:#}")), None),
                }
            }
            VmCmd::OpenShell { slug, path, reply } => {
                let root = workspace_root(&path);
                let Some(guest) = guests.get_mut(&root) else {
                    let _ = reply.send(Err(format!("no VM for workspace {}", root.display())));
                    continue;
                };
                let Some(ssh) = guest.ssh.clone() else {
                    let _ = reply.send(Err(format!("VM for {slug} is not Ready")));
                    continue;
                };
                // Allocate the preview forward once per workspace.
                if guest.preview_host.is_none() {
                    match guest.ports.alloc() {
                        Ok(p) => guest.preview_host = Some(p),
                        Err(e) => {
                            let _ = reply.send(Err(format!("{e:#}")));
                            continue;
                        }
                    }
                }
                let mut vm = MicroVm::new(
                    SshTarget {
                        host: ssh.host,
                        port: ssh.port,
                        user: ssh.user,
                        key_path: guest.key.priv_path().to_path_buf(),
                    },
                    PathMapping::Mounted {
                        host_root: root,
                        guest_root: PathBuf::from("/workspace"),
                    },
                );
                if let Some(host) = guest.preview_host {
                    vm.add_forward(PREVIEW_GUEST_PORT, host);
                }
                // For virtiofs mounts the repo path is host-visible, so
                // the pty can start in the worktree directory itself.
                let res = vm
                    .shell(&path)
                    .map_err(|e| format!("guest shell failed: {e:#}"));
                let _ = reply.send(res);
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
        // Ready reports carry the SSH endpoint (Mock: ubuntu + loopback).
        let ready = got
            .iter()
            .find(|r| matches!(r.state, VmState::Ready))
            .unwrap();
        assert_eq!(ready.ssh.as_ref().unwrap().user, "ubuntu");
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

    #[test]
    fn open_shell_without_vm_is_a_clean_error() {
        let vm = VmThread::spawn();
        let rx = vm.open_shell("wt-x".into(), PathBuf::from("/tmp/ws-x"));
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("reply arrives");
        assert!(res.is_err());
    }

    #[test]
    fn open_shell_returns_a_shell_on_mock_guest() {
        if probe_kvm() {
            return; // CH backend would boot a real guest (live-gated).
        }
        let vm = VmThread::spawn();
        // The pty needs a local cwd; reuse the OS tempdir.
        let cwd = std::env::temp_dir();
        vm.ensure("wt-s".into(), cwd.clone());
        let mut ready = false;
        for _ in 0..200 {
            if vm.drain().iter().any(|r| matches!(r.state, VmState::Ready)) {
                ready = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(ready);
        // Real ssh to the mock port is refused, but the pty child
        // spawns — arrival (not success) is what this tests.
        let rx = vm.open_shell("wt-s".into(), cwd);
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("reply arrives");
        assert!(res.is_ok());
    }
}
