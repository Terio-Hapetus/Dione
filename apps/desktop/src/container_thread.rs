//! Container background thread (ADR-0006, ADR-0007): the UI thread never blocks.
//!
//! Replaces `vm_thread` (MicroVM). One podman container per worktree
//! (ADR-0007, supersedes 1-per-workspace), keyed by the worktree path
//! (canonicalized worktree dir when it exists, else the raw path).
//! The UI sends [`ContainerCmd`] and drains [`ContainerReport`]s in its
//! 160ms loop. No KVM, no SSH keys, no vsock: missing `podman` reports
//! `Error` so the app falls back to Host mode. ADR-0007 adds
//! `Pause`/`Unpause` (cgroup freezer) — sleep for idle worktrees.

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
    Pause {
        slug: String,
        path: PathBuf,
    },
    Unpause {
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
/// else the path itself. Kept for `sandbox/` split compatibility (ADR-0007
/// worktree-keyed containers still mount the repo root).
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

/// Canonical key for a worktree container: real path when the dir
/// exists (stable under symlinks), else the raw path. One container per
/// worktree (ADR-0007), so each worktree path must hash distinctly.
pub(crate) fn worktree_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
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

    pub fn pause(&self, slug: String, path: PathBuf) {
        let _ = self.tx.send(ContainerCmd::Pause { slug, path });
    }

    pub fn unpause(&self, slug: String, path: PathBuf) {
        let _ = self.tx.send(ContainerCmd::Unpause { slug, path });
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
    // Worktree path (canonical) → container name + allocated preview port.
    // ADR-0007: 1 container per worktree, so worktree paths hash distinctly
    // (not collapsed to the repo root like the workspace-keyed P3).
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
                let key = worktree_key(&path);
                let root = workspace_root(&path);
                if !probe_podman() {
                    report(&slug, ContainerState::Error("podman not found".into()));
                    continue;
                }
                // Optimistic badge; the real state follows ensure_running.
                report(&slug, ContainerState::Pulling);
                let preview = guests.get(&key).and_then(|(_, p)| *p).or_else(|| {
                    if next_preview <= PREVIEW_CAP {
                        let p = next_preview;
                        next_preview += 1;
                        Some(p)
                    } else {
                        None
                    }
                });
                let mut spec = ContainerSpec::for_workspace(&root);
                spec.name = workspace::container_name_for(&key);
                spec.preview_host = preview;
                match mgr.ensure_running(&spec) {
                    ContainerState::Running => {
                        guests.insert(key, (spec.name.clone(), preview));
                        report(&slug, ContainerState::Running);
                    }
                    st => report(&slug, st),
                }
            }
            ContainerCmd::Stop { slug, path } => {
                let key = worktree_key(&path);
                guests.remove(&key);
                let name = workspace::container_name_for(&key);
                match mgr.stop(&name) {
                    Ok(true) => report(&slug, ContainerState::Stopped),
                    Ok(false) => report(&slug, ContainerState::Missing),
                    Err(e) => report(&slug, ContainerState::Error(format!("{e:#}"))),
                }
            }
            ContainerCmd::Pause { slug, path } => {
                let key = worktree_key(&path);
                let name = workspace::container_name_for(&key);
                report(&slug, mgr.pause(&name));
            }
            ContainerCmd::Unpause { slug, path } => {
                let key = worktree_key(&path);
                let name = workspace::container_name_for(&key);
                report(&slug, mgr.unpause(&name));
            }
            ContainerCmd::OpenShell { slug, path, reply } => {
                let key = worktree_key(&path);
                let Some((name, _)) = guests.get(&key) else {
                    let _ = reply.send(Err(format!(
                        "no container for {slug} (worktree {})",
                        key.display()
                    )));
                    continue;
                };
                let mut provider = PodmanProvider::new(name, &workspace_root(&path));
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
    fn worktree_key_is_per_worktree_not_per_workspace() {
        // Different worktrees → different keys (not collapsed).
        assert_ne!(
            worktree_key(Path::new("/repo/.dione-worktrees/a")),
            worktree_key(Path::new("/repo/.dione-worktrees/b"))
        );
        // Same worktree → same key.
        assert_eq!(
            worktree_key(Path::new("/repo/.dione-worktrees/a")),
            worktree_key(Path::new("/repo/.dione-worktrees/a"))
        );
        // workspace_root collapses them — the old convergence.
        assert_eq!(
            workspace_root(Path::new("/repo/.dione-worktrees/a")),
            workspace_root(Path::new("/repo/.dione-worktrees/b"))
        );
    }

    #[test]
    fn pause_and_unpause_report_error_without_podman() {
        if probe_podman() {
            return;
        }
        let ctr = ContainerThread::spawn();
        ctr.pause("wt-a".into(), PathBuf::from("/tmp/ws-a/.dione-worktrees/a"));
        ctr.unpause("wt-b".into(), PathBuf::from("/tmp/ws-b/.dione-worktrees/b"));
        let mut got = Vec::new();
        for _ in 0..100 {
            got.extend(ctr.drain());
            if got
                .iter()
                .filter(|r| matches!(r.state, ContainerState::Error(_)))
                .count()
                >= 2
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            got.iter()
                .filter(|r| matches!(r.state, ContainerState::Error(_)))
                .count()
                >= 2,
            "{got:?}"
        );
    }

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
        let rx = ctr.open_shell("wt-x".into(), PathBuf::from("/tmp/ws-x/.dione-worktrees/x"));
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
        ctr.ensure("wt-a".into(), PathBuf::from("/tmp/ws-a/.dione-worktrees/a"));
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
