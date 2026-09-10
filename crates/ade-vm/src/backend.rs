use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};

use thiserror::Error;

use super::config::{SshInfo, VmConfig, VmHandle, VmState};

#[derive(Debug, Error)]
pub enum VmError {
    #[error("unknown vm handle: {0}")]
    UnknownHandle(String),
    #[error("vm {0} already stopped")]
    AlreadyStopped(String),
    #[error("backend failure: {0}")]
    Backend(String),
}

/// Replaceable VM engine: CloudHypervisor for real MicroVMs,
/// Mock keeps CI green without KVM.
pub trait VmBackend: Send {
    fn boot(&mut self, cfg: &VmConfig) -> anyhow::Result<VmHandle>;
    fn wait_ssh(&self, handle: &VmHandle) -> anyhow::Result<SshInfo>;
    fn state_of(&self, handle: &VmHandle) -> VmState;
    fn stop(&mut self, handle: &VmHandle) -> anyhow::Result<()>;
}

/// Returns true when `/dev/kvm` exists. No KVM → the app must fall back
/// to Host mode with a banner, never crash.
pub fn probe_kvm() -> bool {
    Path::new("/dev/kvm").exists()
}

/// In-memory backend: instant `Missing → Ready` transitions, ephemeral
/// loopback SSH ports. For CI, Xvfb smoke, and machines without KVM.
#[derive(Debug, Default)]
pub struct MockBackend {
    states: BTreeMap<String, VmState>,
    ports: BTreeMap<String, u16>,
    next_id: AtomicU64,
    next_port: AtomicU16,
}

impl MockBackend {
    pub fn new() -> Self {
        Self {
            states: BTreeMap::new(),
            ports: BTreeMap::new(),
            next_id: AtomicU64::new(1),
            next_port: AtomicU16::new(4100),
        }
    }
}

impl VmBackend for MockBackend {
    fn boot(&mut self, _cfg: &VmConfig) -> anyhow::Result<VmHandle> {
        let id = format!("mock-vm-{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let port = self.next_port.fetch_add(1, Ordering::SeqCst);
        self.ports.insert(id.clone(), port);
        // Instant walk through the boot states; a real backend would
        // publish each step with its own timeout (boot 30s, ssh 20s...).
        self.states.insert(id.clone(), VmState::Ready);
        Ok(VmHandle { id })
    }

    fn wait_ssh(&self, handle: &VmHandle) -> anyhow::Result<SshInfo> {
        let port = self
            .ports
            .get(&handle.id)
            .copied()
            .ok_or_else(|| VmError::UnknownHandle(handle.id.clone()))?;
        Ok(SshInfo {
            host: "127.0.0.1".into(),
            port,
            user: "vm".into(),
        })
    }

    fn state_of(&self, handle: &VmHandle) -> VmState {
        self.states
            .get(&handle.id)
            .cloned()
            .unwrap_or(VmState::Missing)
    }

    fn stop(&mut self, handle: &VmHandle) -> anyhow::Result<()> {
        match self.states.get(&handle.id) {
            None => Err(VmError::UnknownHandle(handle.id.clone()).into()),
            Some(s) if s.is_terminal() => Err(VmError::AlreadyStopped(handle.id.clone()).into()),
            _ => {
                self.states.insert(handle.id.clone(), VmState::Stopped);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> VmConfig {
        VmConfig::default()
    }

    #[test]
    fn mock_full_lifecycle() {
        let mut b = MockBackend::new();
        let h = b.boot(&cfg()).unwrap();
        assert_eq!(b.state_of(&h), VmState::Ready);
        let ssh = b.wait_ssh(&h).unwrap();
        assert_eq!(ssh.host, "127.0.0.1");
        assert_eq!(ssh.user, "vm");
        b.stop(&h).unwrap();
        assert_eq!(b.state_of(&h), VmState::Stopped);
        // Double stop is an error, not a silent no-op.
        assert!(b.stop(&h).is_err());
    }

    #[test]
    fn unknown_handle_errors() {
        let mut b = MockBackend::new();
        let h = VmHandle { id: "nope".into() };
        assert_eq!(b.state_of(&h), VmState::Missing);
        assert!(b.wait_ssh(&h).is_err());
        assert!(b.stop(&h).is_err());
    }

    #[test]
    fn each_boot_gets_own_port() {
        let mut b = MockBackend::new();
        let a = b.boot(&cfg()).unwrap();
        let c = b.boot(&cfg()).unwrap();
        assert_ne!(b.wait_ssh(&a).unwrap().port, b.wait_ssh(&c).unwrap().port);
    }

    #[test]
    fn probe_never_panics() {
        // Either answer is fine; the point is Host fallback, not KVM.
        let _ = probe_kvm();
    }
}
