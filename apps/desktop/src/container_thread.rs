//! Container background thread (ADR-0006): the UI thread never blocks.
//!
//! Replaces `vm_thread` (MicroVM). One podman container per workspace,
//! keyed by [`workspace_root`]. The UI sends [`ContainerCmd`] and drains
//! [`ContainerReport`]s in its 160ms loop. No KVM, no SSH keys, no vsock:
//! missing `podman` reports `Error` so the app falls back to Host mode.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use workspace::{
    ContainerManager, ContainerSpec, ContainerState, PodmanProvider, ShellChannel,
    WorkspaceProvider, probe_podman,
};

/// UI → container thread.
pub enum ContainerCmd {
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

/// Result of `OpenShell`: a live container shell or a human-readable error.
/// `Box` is `Send`, so it crosses threads through the channel.
pub type ShellReply = Result<Box<dyn ShellChannel>, String>;

/// Container thread → UI. No SSH endpoint: containers are not SSH guests.
#[derive(Debug, Clone)]
pub struct ContainerReport {
    pub slug: String,
    pub state: ContainerState,
}

/// Workspace root for a checkout: parent of `.dione-worktrees` when nested,
/// else the path itself. Containers converge to 1-per-workspace; until
/// then each worktree maps to its repo this way.
pub(crate) fn workspace_root(path: &Path) -> PathBuf {
    let mut cur: Option<&Path> = Some(path);
    while let Some(p) = cur {
        if p.file_name().is_some_and(|n| n == ".dione-worktrees")
            && let Some(parent) = p.parent()
        {
            return parent.to_path_buf();
        }
        cur = p.parent();
    }
    path.to_path_buf()
}

/// Preview host-port range (container `:3000` → host `41xx`).
const PREVIEW_BASE: u16 = 4100;
const PREVIEW_CAP: u16 = 4199;

/// Handle the UI owns: send commands, drain reports (non-blocking).
pub struct ContainerThread {
    tx: Sender<ContainerCmd>,
    rx: Receiver<ContainerReport>,
}

impl ContainerThread {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = channel::<ContainerCmd>();
        let (rep_tx, rep_rx) = channel::<ContainerReport>();
        std::thread::spawn(move || container_loop(cmd_rx, rep_tx));
        Self {
            tx: cmd_tx,
            rx: rep_rx,
        }
    }

    pub fn ensure(&self, slug: String, path: PathBuf) {
        let _ = self.tx.send(ContainerCmd::Ensure { slug, path });
    }

    pub fn stop(&self, slug: String, path: PathBuf) {
        let _ = self.tx.send(ContainerCmd::Stop { slug, path });
    }

    /// Ask the thread for an interactive container shell. The reply
    /// arrives on the returned channel; the UI polls it without blocking.
    pub fn open_shell(&self, slug: String, path: PathBuf) -> Receiver<ShellReply> {
        let (tx, rx) = channel::<ShellReply>();
        let _ = self.tx.send(ContainerCmd::OpenShell {
            slug,
            path,
            reply: tx,
        });
        rx
    }

    pub fn drain(&self) -> Vec<ContainerReport> {
        self.rx.try_iter().collect()
    }
}

fn container_loop(cmd_rx: Receiver<ContainerCmd>, rep_tx: Sender<ContainerReport>) {
    let mgr = ContainerManager::new();
    // Workspace root → container name + allocated preview port.
    let mut guests: BTreeMap<PathBuf, (String, Option<u16>)> = BTreeMap::new();
    let mut next_preview: u16 = PREVIEW_BASE;

    let report = |slug: &str, state: ContainerState| {
        let _ = rep_tx.send(ContainerReport {
            slug: slug.to_string(),
            state,
        });
    };

    for cmd in cmd_rx {
        match cmd {
            ContainerCmd::Ensure { slug, path } => {
                let root = workspace_root(&path);
                if !probe_podman() {
                    report(&slug, ContainerState::Error("podman not found".into()));
                    continue;
                }
                // Optimistic badge; the real state follows ensure_running.
                report(&slug, ContainerState::Pulling);
                let preview = guests.get(&root).and_then(|(_, p)| *p).or_else(|| {
                    if next_preview <= PREVIEW_CAP {
                        let p = next_preview;
                        next_preview += 1;
                        Some(p)
                    } else {
                        None
                    }
                });
                let mut spec = ContainerSpec::for_workspace(&root);
                spec.preview_host = preview;
                match mgr.ensure_running(&spec) {
                    ContainerState::Running => {
                        guests.insert(root, (spec.name.clone(), preview));
                        report(&slug, ContainerState::Running);
                    }
                    st => report(&slug, st),
                }
            }
            ContainerCmd::Stop { slug, path } => {
                let root = workspace_root(&path);
                guests.remove(&root);
                let name = ContainerSpec::for_workspace(&root).name;
                match mgr.stop(&name) {
                    Ok(true) => report(&slug, ContainerState::Stopped),
                    Ok(false) => report(&slug, ContainerState::Missing),
                    Err(e) => report(&slug, ContainerState::Error(format!("{e:#}"))),
                }
            }
            ContainerCmd::OpenShell { slug, path, reply } => {
                let root = workspace_root(&path);
                let Some((name, _)) = guests.get(&root) else {
                    let _ = reply.send(Err(format!(
                        "no container for {slug} (workspace {})",
                        root.display()
                    )));
                    continue;
                };
                let mut provider = PodmanProvider::new(name, &root);
                let res = provider
                    .shell(&path)
                    .map_err(|e| format!("container shell failed: {e:#}"));
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
            workspace_root(Path::new("/repo/.dione-worktrees/feat-a")),
            PathBuf::from("/repo")
        );
        assert_eq!(
            workspace_root(Path::new("/repo/.dione-worktrees")),
            PathBuf::from("/repo")
        );
        assert_eq!(workspace_root(Path::new("/repo")), PathBuf::from("/repo"));
    }

    #[test]
    fn open_shell_without_container_is_a_clean_error() {
        let ctr = ContainerThread::spawn();
        let rx = ctr.open_shell("wt-x".into(), PathBuf::from("/tmp/ws-x"));
        let res = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("reply arrives");
        assert!(res.is_err());
    }

    #[test]
    fn ensure_without_podman_reports_error() {
        if probe_podman() {
            return; // Real podman would pull images (live-gated, not in unit tests).
        }
        let ctr = ContainerThread::spawn();
        ctr.ensure("wt-a".into(), PathBuf::from("/tmp/ws-a"));
        let mut got = Vec::new();
        for _ in 0..100 {
            got.extend(ctr.drain());
            if got
                .iter()
                .any(|r| matches!(r.state, ContainerState::Error(_)))
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            got.iter()
                .any(|r| matches!(r.state, ContainerState::Error(_))),
            "{got:?}"
        );
    }
}
