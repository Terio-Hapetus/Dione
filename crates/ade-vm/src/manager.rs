use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::backend::{VmBackend, VmError};
use super::config::{VmConfig, VmHandle, VmState};

/// Per-state timeouts (spec: boot 30s, ssh 20s, mount 10s).
#[derive(Debug, Clone, Copy)]
pub struct VmTimeouts {
    pub boot: Duration,
    pub ssh: Duration,
    pub mount: Duration,
    pub poll: Duration,
}

impl Default for VmTimeouts {
    fn default() -> Self {
        Self {
            boot: Duration::from_secs(30),
            ssh: Duration::from_secs(20),
            mount: Duration::from_secs(10),
            poll: Duration::from_millis(500),
        }
    }
}

impl VmTimeouts {
    pub fn test() -> Self {
        Self {
            boot: Duration::from_millis(200),
            ssh: Duration::from_millis(200),
            mount: Duration::from_millis(200),
            poll: Duration::from_millis(5),
        }
    }
}

/// 1 VM per workspace (ADR-0002), keyed by canonical repo path.
/// Backend-agnostic: image prep lives inside `backend.boot`,
/// mount verification arrives as a caller closure (touch via the
/// workspace provider, both directions). Failures land in
/// `VmState::Error` and are kept for manual retry — never auto-pruned.
pub struct VmManager<B: VmBackend> {
    backend: B,
    vms: BTreeMap<PathBuf, ManagedVm>,
    timeouts: VmTimeouts,
}

#[derive(Debug, Clone)]
struct ManagedVm {
    handle: VmHandle,
    state: VmState,
}

impl<B: VmBackend> VmManager<B> {
    pub fn new(backend: B) -> Self {
        Self::with_timeouts(backend, VmTimeouts::default())
    }

    pub fn with_timeouts(backend: B, timeouts: VmTimeouts) -> Self {
        Self {
            backend,
            vms: BTreeMap::new(),
            timeouts,
        }
    }

    pub fn state_of_workspace(&self, workspace: &Path) -> VmState {
        let key = canon(workspace);
        self.vms
            .get(&key)
            .map(|m| m.state.clone())
            .unwrap_or(VmState::Missing)
    }

    /// Current SSH endpoint for a managed workspace (one probe, no retry).
    /// Providers are built from this after `ensure_ready`.
    pub fn ssh_info(&self, workspace: &Path) -> anyhow::Result<super::config::SshInfo> {
        let key = canon(workspace);
        let m = self
            .vms
            .get(&key)
            .ok_or_else(|| anyhow::anyhow!("no vm for {}", key.display()))?;
        self.backend.wait_ssh(&m.handle)
    }

    /// Boot (if needed) → wait SSH → verify mount → Ready.
    /// Idempotent: a Ready/Running VM is returned as-is.
    pub fn ensure_ready(
        &mut self,
        workspace: &Path,
        cfg: &VmConfig,
        mut check_mount: impl FnMut() -> anyhow::Result<()>,
    ) -> anyhow::Result<VmHandle> {
        let key = canon(workspace);
        if let Some(m) = self.vms.get(&key)
            && matches!(
                self.backend.state_of(&m.handle),
                VmState::Ready | VmState::Running
            )
        {
            return Ok(m.handle.clone());
        }
        self.set_state(&key, VmState::PullingImage);
        self.set_state(&key, VmState::Booting);
        let handle = match self.backend.boot(cfg) {
            Ok(h) => h,
            Err(e) => {
                let msg = format!("boot failed: {e:#}");
                self.set_state(&key, VmState::Error(msg.clone()));
                anyhow::bail!("{msg}");
            }
        };
        self.vms.insert(
            key.clone(),
            ManagedVm {
                handle: handle.clone(),
                state: VmState::Booting,
            },
        );
        // wait_ssh is one probe per call; poll until the ssh timeout.
        self.set_state(&key, VmState::WaitingSsh);
        let start = Instant::now();
        loop {
            match self.backend.wait_ssh(&handle) {
                Ok(_) => break,
                Err(e) if start.elapsed() < self.timeouts.ssh => {
                    let _ = e;
                    std::thread::sleep(self.timeouts.poll);
                }
                Err(e) => {
                    let msg = format!("ssh timeout: {e:#}");
                    self.set_state(&key, VmState::Error(msg.clone()));
                    anyhow::bail!("{msg}");
                }
            }
        }
        self.set_state(&key, VmState::Mounting);
        let mount_start = Instant::now();
        loop {
            match check_mount() {
                Ok(()) => break,
                Err(e) if mount_start.elapsed() < self.timeouts.mount => {
                    let _ = e;
                    std::thread::sleep(self.timeouts.poll);
                }
                Err(e) => {
                    let msg = format!("mount failed: {e:#}");
                    self.set_state(&key, VmState::Error(msg.clone()));
                    anyhow::bail!("{msg}");
                }
            }
        }
        self.set_state(&key, VmState::Ready);
        Ok(handle)
    }

    /// Stop + forget. `Ok(false)` = nothing was running.
    pub fn stop(&mut self, workspace: &Path) -> anyhow::Result<bool> {
        let key = canon(workspace);
        let Some(m) = self.vms.remove(&key) else {
            return Ok(false);
        };
        match self.backend.stop(&m.handle) {
            Ok(()) => Ok(true),
            Err(e) => {
                // Backend already forgot it: treat as stopped.
                if is_unknown(&e) {
                    Ok(false)
                } else {
                    self.vms.insert(
                        key,
                        ManagedVm {
                            handle: m.handle,
                            state: VmState::Error(format!("stop failed: {e:#}")),
                        },
                    );
                    anyhow::bail!("stop failed: {e:#}");
                }
            }
        }
    }

    fn set_state(&mut self, key: &Path, state: VmState) {
        if let Some(m) = self.vms.get_mut(key) {
            m.state = state;
        } else {
            // Pre-boot states have no handle yet; keep a placeholder so
            // the UI badge can show PullingImage/Booting immediately.
            self.vms.insert(
                key.to_path_buf(),
                ManagedVm {
                    handle: VmHandle { id: String::new() },
                    state,
                },
            );
        }
    }
}

fn canon(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

fn is_unknown(e: &anyhow::Error) -> bool {
    e.downcast_ref::<VmError>()
        .is_some_and(|v| matches!(v, VmError::UnknownHandle(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SshInfo;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Backend stub: instant boot, scripted ssh/mount behavior.
    struct Stub {
        boots: AtomicUsize,
        ssh_ok: bool,
    }

    impl VmBackend for Stub {
        fn boot(&mut self, _cfg: &VmConfig) -> anyhow::Result<VmHandle> {
            self.boots.fetch_add(1, Ordering::SeqCst);
            Ok(VmHandle {
                id: "stub-1".into(),
            })
        }
        fn wait_ssh(&self, h: &VmHandle) -> anyhow::Result<SshInfo> {
            if self.ssh_ok {
                Ok(SshInfo {
                    host: "127.0.0.1".into(),
                    port: 4222,
                    // Same as the live guest user (seed.rs).
                    user: "ubuntu".into(),
                })
            } else {
                Err(VmError::UnknownHandle(h.id.clone()).into())
            }
        }
        fn state_of(&self, _h: &VmHandle) -> VmState {
            VmState::Ready
        }
        fn stop(&mut self, _h: &VmHandle) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn key(p: &str) -> PathBuf {
        PathBuf::from(p)
    }

    #[test]
    fn happy_path_and_idempotent() {
        let mut m = VmManager::with_timeouts(
            Stub {
                boots: AtomicUsize::new(0),
                ssh_ok: true,
            },
            VmTimeouts::test(),
        );
        let cfg = VmConfig::default();
        let h1 = m.ensure_ready(&key("/repo/a"), &cfg, || Ok(())).unwrap();
        let h2 = m.ensure_ready(&key("/repo/a"), &cfg, || Ok(())).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(m.backend.boots.load(Ordering::SeqCst), 1);
        assert_eq!(m.state_of_workspace(&key("/repo/a")), VmState::Ready);
        assert!(m.stop(&key("/repo/a")).unwrap());
        assert!(!m.stop(&key("/repo/a")).unwrap());
        assert_eq!(m.state_of_workspace(&key("/repo/a")), VmState::Missing);
    }

    #[test]
    fn ssh_timeout_records_error() {
        let mut m = VmManager::with_timeouts(
            Stub {
                boots: AtomicUsize::new(0),
                ssh_ok: false,
            },
            VmTimeouts::test(),
        );
        let err = m
            .ensure_ready(&key("/repo/b"), &VmConfig::default(), || Ok(()))
            .unwrap_err();
        assert!(err.to_string().contains("ssh timeout"));
        assert!(matches!(
            m.state_of_workspace(&key("/repo/b")),
            VmState::Error(_)
        ));
    }

    #[test]
    fn mount_failure_records_error() {
        let mut m = VmManager::with_timeouts(
            Stub {
                boots: AtomicUsize::new(0),
                ssh_ok: true,
            },
            VmTimeouts::test(),
        );
        let err = m
            .ensure_ready(&key("/repo/c"), &VmConfig::default(), || {
                anyhow::bail!("no virtiofs")
            })
            .unwrap_err();
        assert!(err.to_string().contains("mount failed"));
        assert!(matches!(
            m.state_of_workspace(&key("/repo/c")),
            VmState::Error(_)
        ));
    }
}
