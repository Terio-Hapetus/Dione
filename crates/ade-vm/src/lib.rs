//! 1 MicroVM per workspace (M5 kickoff): config types, the shared
//! `VmState` machine, the replaceable [`VmBackend`] socket, and a
//! [`MockBackend`] so CI/Xvfb machines without `/dev/kvm` stay green.
//!
//! Spec: `docs/WORKSPACE-VM.md`. No KVM → Host fallback + banner.

pub mod backend;
pub mod ch;
pub mod config;
pub mod image;
pub mod keys;
pub mod manager;
pub mod sbx;
pub mod seed;
pub mod uds;
pub mod vsock_proxy;

pub use backend::{MockBackend, VmBackend, VmError, probe_kvm};
pub use ch::{CloudHypervisorBackend, build_vm_config, resolve_assets};
pub use config::{ImageRef, NetPolicy, SshInfo, VmConfig, VmHandle, VmState};
pub use image::{ImageSpec, default_cache_dir, ensure_image, verify_sha256};
pub use keys::EphemeralKey;
pub use manager::{VmManager, VmTimeouts};
pub use sbx::ExternalSbxBackend;
