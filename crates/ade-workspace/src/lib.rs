//! Workspace seam (M4 kickoff): 1 repo + config + worktrees.
//! Agents see only [`WorkspaceProvider`], never Host-vs-VM.
//!
//! Depends one-way on `ade-core` (TaskId) and `ade-vm` (SshInfo).
//! `MicroVm` provider + pty `shell()` land in M5–M6.

pub mod host;
pub mod microvm;
pub mod provider;
pub mod task;

pub use host::HostProvider;
pub use microvm::{MicroVm, PathMapping, SshTarget, shell_quote};
pub use provider::{ExecOut, MockWorkspace, WorkspaceProvider};
pub use task::Task;
