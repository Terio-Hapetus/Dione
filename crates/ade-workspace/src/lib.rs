//! Workspace seam (M4 kickoff): 1 repo + config + worktrees.
//! Agents see only [`WorkspaceProvider`], never Host-vs-VM.
//!
//! Depends one-way on `ade-core` (TaskId) and `ade-vm` (SshInfo).
//! `MicroVm` provider + pty `shell()` land in M5–M6.

pub mod agents;
pub mod dispatcher;
pub mod host;
pub mod microvm;
pub mod provider;
pub mod task;

pub use agents::{AgentEntry, default_agents_path, load_agents_toml, probe_all, probe_bin};
pub use dispatcher::{DISPATCH_INTERVAL_SECS, Dispatcher, DispatcherAction};
pub use host::{HostProvider, HostShell};
pub use microvm::{MicroVm, PathMapping, PreviewPorts, SshTarget, shell_quote};
pub use provider::{ExecOut, MockWorkspace, ShellChannel, WorkspaceProvider, strip_ansi};
pub use task::{DEFAULT_FAILURE_LIMIT, Task, TaskStatus, handoff_summary};
