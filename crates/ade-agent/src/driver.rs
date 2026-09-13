//! M6b driver: Open-Workspace flow, Host-only.
//!
//! Turns `(slug, agent_ref, prompt)` into a registered [`Supervisor`].
//! The runtime then binds it to the session created in the matching
//! worktree scope (`bind_new`) and drains it every poll tick (hook A).
//! VM workspaces (`MicroVm`) plug into the same entry point later.
//!
//! Takes `&FleetInbox` (not `&RuntimeHandle`) so tests run without a
//! server thread; production passes `rt.fleet()`.

use std::path::Path;

use ade_core::runtime::FleetInbox;
use ade_core::transcript::TaskId;
use ade_workspace::{HostProvider, Task, WorkspaceProvider};

use super::agent::{AgentBackend, MockAgent, OpencodeAdapter};
use super::supervisor::Supervisor;
use super::sweeper::FleetSweeper;
use super::terminal::TerminalAdapter;

/// Open one host task and register it (plus the sweeper, once).
/// `agent_ref`: `"mock"` (scripted) or `"opencode"` (read-side adapter).
pub fn open_host_task(
    inbox: &FleetInbox,
    slug: &str,
    agent_ref: &str,
    prompt: &str,
) -> anyhow::Result<TaskId> {
    open_task_with(
        inbox,
        slug,
        agent_ref,
        prompt,
        Path::new("/tmp"),
        Box::new(HostProvider::new()),
    )
}

/// Open one task on any provider. `cwd` is the shell directory for the
/// `"terminal"` backend (ignored otherwise). `agent_ref`: `"mock"` |
/// `"opencode"` | `"terminal"` (any CLI over `ws.shell`).
pub fn open_task_with(
    inbox: &FleetInbox,
    slug: &str,
    agent_ref: &str,
    prompt: &str,
    cwd: &Path,
    ws: Box<dyn WorkspaceProvider>,
) -> anyhow::Result<TaskId> {
    open_registered_task(inbox, Task::new(slug, agent_ref), prompt, cwd, ws)
}

/// Open a continuation task (M7c handoff): fresh id and budget, parent
/// linked, summary inherited. `slug` is the child's own worktree slug.
pub fn open_child_task(
    inbox: &FleetInbox,
    parent: &Task,
    slug: &str,
    agent_ref: &str,
    prompt: &str,
    cwd: &Path,
    ws: Box<dyn WorkspaceProvider>,
) -> anyhow::Result<TaskId> {
    open_registered_task(inbox, parent.child(slug, agent_ref), prompt, cwd, ws)
}

fn open_registered_task(
    inbox: &FleetInbox,
    task: Task,
    prompt: &str,
    cwd: &Path,
    mut ws: Box<dyn WorkspaceProvider>,
) -> anyhow::Result<TaskId> {
    // M7d 1:1 — one subtask per worktree. Fail before building backends
    // so a duplicate open has no side effects (notably no stray shell).
    if inbox.has_slug(&task.slug) {
        anyhow::bail!("task already open for slug {:?}", task.slug);
    }
    if !inbox.has_sweeper() {
        inbox.register_sweeper(Box::new(FleetSweeper::new()));
    }
    let id = task.id;
    let mut backend: Box<dyn AgentBackend> = match task.agent_ref.as_str() {
        "mock" => Box::new(MockAgent::new()),
        "opencode" => Box::new(OpencodeAdapter::new()),
        "terminal" => {
            let shell = ws.shell(cwd)?;
            let mut adapter = TerminalAdapter::new();
            adapter.attach(id, shell);
            Box::new(adapter)
        }
        other => anyhow::bail!("unknown agent_ref {other:?} (mock|opencode|terminal)"),
    };
    // Seed the first prompt where the backend accepts session I/O;
    // read-side adapters pick the prompt up from the session instead.
    let _ = backend.spawn(id, prompt);
    // Belt and suspenders with the pre-check above: a lost race still
    // fails instead of registering a ghost task.
    if !inbox.register_task(Box::new(Supervisor::new(task, backend, ws))) {
        anyhow::bail!("task already open (lost registration race)");
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ade_core::state::Store;
    use ade_core::transcript::Role;
    use ade_workspace::{ExecOut, MockWorkspace, ShellChannel};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    #[test]
    fn mock_task_drains_prompt_into_transcript() {
        let inbox = FleetInbox::new();
        let id = open_host_task(&inbox, "wt-a", "mock", "hello").unwrap();
        assert_eq!(inbox.task_count(), 1);
        assert!(inbox.has_sweeper());
        let mut store = Store::default();
        inbox.poll_fleet(&mut store, 1);
        let msgs = store.transcripts.get(&id).cloned().unwrap_or_default();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "hello");
    }

    #[test]
    fn unknown_agent_is_rejected() {
        let inbox = FleetInbox::new();
        assert!(open_host_task(&inbox, "wt-a", "claude", "hi").is_err());
        assert_eq!(inbox.task_count(), 0);
    }

    #[test]
    fn child_task_flows_with_handoff_context() {
        let inbox = FleetInbox::new();
        let parent = Task::new("wt-p", "mock")
            .with_summary("did X, stuck on Y")
            .with_failure_limit(3);
        let pid = parent.id;
        let ws: Box<dyn WorkspaceProvider> = Box::new(MockWorkspace::new());
        let cid = open_child_task(
            &inbox,
            &parent,
            "wt-c",
            "mock",
            "continue",
            Path::new("/tmp"),
            ws,
        )
        .unwrap();
        assert_ne!(cid, pid);
        assert_eq!(inbox.task_count(), 1);
        // The child's prompt drains under its own fresh id.
        let mut store = Store::default();
        inbox.poll_fleet(&mut store, 1);
        let msgs = store.transcripts.get(&cid).cloned().unwrap_or_default();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "continue");
    }

    /// Provider handing out scripted shells (no pty, no network).
    struct FakeProvider {
        outputs: Arc<Mutex<VecDeque<Vec<u8>>>>,
    }

    struct FakeShell {
        outputs: Arc<Mutex<VecDeque<Vec<u8>>>>,
    }

    impl WorkspaceProvider for FakeProvider {
        fn exec(&mut self, _cmd: &[&str], _cwd: &Path) -> anyhow::Result<ExecOut> {
            anyhow::bail!("no exec in fake")
        }

        fn shell(&mut self, _cwd: &Path) -> anyhow::Result<Box<dyn ShellChannel>> {
            Ok(Box::new(FakeShell {
                outputs: Arc::clone(&self.outputs),
            }))
        }
    }

    impl ShellChannel for FakeShell {
        fn write_bytes(&mut self, _data: &[u8]) -> anyhow::Result<()> {
            // Answer every prompt like a CLI would (with ANSI colors).
            self.outputs
                .lock()
                .unwrap()
                .push_back(b"\x1b[32mdone-it\x1b[0m\n".to_vec());
            Ok(())
        }
        fn read_available(&mut self) -> Vec<u8> {
            self.outputs.lock().unwrap().drain(..).flatten().collect()
        }
        fn resize(&mut self, _cols: u16, _rows: u16) -> anyhow::Result<()> {
            Ok(())
        }
        fn is_alive(&mut self) -> bool {
            true
        }
        fn kill(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn fan_out_one_prompt_to_two_tasks() {
        let inbox = FleetInbox::new();
        let a = open_host_task(&inbox, "wt-a", "mock", "same prompt").unwrap();
        let b = open_host_task(&inbox, "wt-b", "mock", "same prompt").unwrap();
        assert_ne!(a, b);
        // Sessions created later bind by slug; sweeper tracks both.
        inbox.bind_new("wt-a", "s1");
        inbox.bind_new("wt-b", "s2");
        let mut store = Store::default();
        inbox.poll_fleet(&mut store, 1);
        for id in [a, b] {
            let msgs = store.transcripts.get(&id).cloned().unwrap_or_default();
            assert_eq!(msgs.len(), 1);
            assert_eq!(msgs[0].text, "same prompt");
        }
        // Sweeper installed once, still quiet for fresh tasks.
        inbox.poll_fleet(&mut store, 30);
        assert!(store.errors.is_empty());
    }

    #[test]
    fn duplicate_slug_open_fails_without_side_effects() {
        let inbox = FleetInbox::new();
        open_host_task(&inbox, "wt-a", "mock", "first").unwrap();
        assert_eq!(inbox.task_count(), 1);
        let err = open_host_task(&inbox, "wt-a", "mock", "second").unwrap_err();
        assert!(err.to_string().contains("already open"));
        assert_eq!(inbox.task_count(), 1);
        // A fresh slug still opens.
        open_host_task(&inbox, "wt-b", "mock", "other").unwrap();
        assert_eq!(inbox.task_count(), 2);
    }

    #[test]
    fn terminal_task_flows_through_fake_shell() {
        let inbox = FleetInbox::new();
        let ws: Box<dyn WorkspaceProvider> = Box::new(FakeProvider {
            outputs: Arc::new(Mutex::new(VecDeque::new())),
        });
        // Shell failure surfaces instead of registering half a task.
        let mut bad_ws: Box<dyn WorkspaceProvider> = Box::new(MockWorkspace::new());
        assert!(bad_ws.shell(Path::new("/tmp")).is_err());
        let id =
            open_task_with(&inbox, "wt-t", "terminal", "do it", Path::new("/tmp"), ws).unwrap();
        let mut store = Store::default();
        inbox.poll_fleet(&mut store, 1);
        let msgs = store.transcripts.get(&id).cloned().unwrap_or_default();
        assert!(
            msgs.iter()
                .any(|m| m.role == Role::User && m.text == "do it")
        );
        let agent_text: Vec<&str> = msgs
            .iter()
            .filter(|m| m.role == Role::Agent)
            .map(|m| m.text.as_str())
            .collect();
        assert_eq!(agent_text, vec!["done-it\n"]);
    }
}
