use std::path::PathBuf;

/// VM image reference, e.g. `"ade-ubuntu-24.04:v1"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef(pub String);

/// Network policy. Only `Open` is enforced for now (p1); the other
/// variants reserve the proxy hook for later milestones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetPolicy {
    #[default]
    Open,
    Balanced,
    Locked,
}

/// Boot configuration: 1 VM per workspace.
#[derive(Debug, Clone)]
pub struct VmConfig {
    pub vcpu: u8,
    pub mem_mb: u32,
    pub image: ImageRef,
    pub mount_repo: PathBuf,
    pub net: NetPolicy,
    /// Ephemeral public key, generated fresh per boot, never reused.
    pub ssh_pubkey: String,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            vcpu: 2,
            mem_mb: 2048,
            image: ImageRef("ade-ubuntu-24.04:v1".into()),
            mount_repo: PathBuf::from("."),
            net: NetPolicy::Open,
            ssh_pubkey: String::new(),
        }
    }
}

/// Handle to a booted VM. Cheap to clone; the backend owns the state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VmHandle {
    pub id: String,
}

/// How the host reaches the guest SSH server (key is ephemeral per boot
/// and lives only in memory on the host side).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshInfo {
    pub host: String,
    pub port: u16,
    pub user: String,
}

/// Shared lifecycle state machine for UI badges and backend code.
/// Terminal states: `Stopped`, `Error`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum VmState {
    #[default]
    Missing,
    PullingImage,
    Booting,
    WaitingSsh,
    Mounting,
    Ready,
    Running,
    Stopped,
    Error(String),
}

impl VmState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Stopped | Self::Error(_))
    }
}
