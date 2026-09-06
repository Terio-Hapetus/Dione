//! 1 MicroVM per workspace (M5 kickoff): config types, the shared
//! `VmState` machine, the replaceable [`VmBackend`] socket, and a
//! [`MockBackend`] so CI/Xvfb machines without `/dev/kvm` stay green.
//!
//! Spec: `docs/WORKSPACE-VM.md`. No KVM → Host fallback + banner.

pub mod backend;
pub mod config;

pub use backend::{MockBackend, VmBackend, VmError, probe_kvm};
pub use config::{ImageRef, NetPolicy, SshInfo, VmConfig, VmHandle, VmState};
