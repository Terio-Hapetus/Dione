//! Workspace seam (M4 kickoff): 1 repo + config + worktrees.
//! Agents see only [`WorkspaceProvider`], never Host-vs-container.
//!
//! Depends one-way on `base` (TaskId).
//! `PodmanProvider` + pty `shell()` land in podman P1–P3 (ADR-0006).

pub mod agents;
pub mod containers;
pub mod dispatcher;
pub mod host;
pub mod podman;
pub mod provider;
pub mod secrets;
pub mod task;
pub mod usage_probe;

pub use agents::{AgentEntry, default_agents_path, load_agents_toml, probe_all, probe_bin};
pub use containers::{
    CTR_IMAGE, ContainerManager, ContainerSpec, ContainerState, PREVIEW_CTR_PORT,
    container_name_for,
};
pub use dispatcher::{DISPATCH_INTERVAL_SECS, Dispatcher, DispatcherAction};
pub use host::{HostProvider, HostShell};
pub use podman::{CTR_WORKSPACE, ContainerMount, PodmanProvider, probe_podman};
pub use provider::{ExecOut, MockWorkspace, ShellChannel, WorkspaceProvider, strip_ansi};
pub use secrets::{
    CliSecrets, MockSecrets, ResolvedEnv, SECRET_SERVICE, Secrets, probe_secret_tool,
    resolve_task_env, secret_account,
};
pub use task::{DEFAULT_FAILURE_LIMIT, Task, TaskStatus, handoff_summary};
pub use usage_probe::{
    LogUsage, QuotaWindow, claude_log_dir, codex_log_dir, parse_claude_jsonl, parse_codex_jsonl,
    samples_for, scan_jsonl_logs,
};
