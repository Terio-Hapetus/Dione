//! M6b driver: Open-Workspace flow, Host-only.
//!
//! Turns `(slug, agent_ref, prompt)` into a registered [`Supervisor`]
//! backed by [`HostProvider`]. The runtime then binds it to the session
//! created in the matching worktree scope (`bind_new`) and drains it
//! every poll tick (hook A). VM workspaces (`MicroVm`) and interactive
//! CLIs (`TerminalAdapter`) plug into this same entry point later.
//!
//! Takes `&FleetInbox` (not `&RuntimeHandle`) so tests run without a
//! server thread; production passes `rt.fleet()`.

use ade_core::runtime::FleetInbox;
use ade_core::transcript::TaskId;
use ade_workspace::{HostProvider, Task};

use super::agent::{AgentBackend, MockAgent, OpencodeAdapter};
use super::supervisor::Supervisor;
use super::sweeper::FleetSweeper;

/// Open one host task and register it (plus the sweeper, once).
/// `agent_ref`: `"mock"` (scripted) or `"opencode"` (read-side adapter).
pub fn open_host_task(
    inbox: &FleetInbox,
    slug: &str,
    agent_ref: &str,
    prompt: &str,
) -> anyhow::Result<TaskId> {
    if !inbox.has_sweeper() {
        inbox.register_sweeper(Box::new(FleetSweeper::new()));
    }
    let task = Task::new(slug, agent_ref);
    let id = task.id;
    let mut backend: Box<dyn AgentBackend> = match agent_ref {
        "mock" => Box::new(MockAgent::new()),
        "opencode" => Box::new(OpencodeAdapter::new()),
        other => anyhow::bail!("unknown agent_ref {other:?} (m6b: mock|opencode)"),
    };
    // Seed the first prompt where the backend accepts session I/O;
    // read-side adapters pick the prompt up from the session instead.
    let _ = backend.spawn(id, prompt);
    inbox.register_task(Box::new(Supervisor::new(
        task,
        backend,
        Box::new(HostProvider::new()),
    )));
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ade_core::state::Store;

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
}
