//! M4c supervisor: one task = one backend + one workspace face.
//!
//! Legacy `runtime.rs` is untouched; the runtime (M4d dispatcher) drives
//! `tick`, which only drains `backend.poll()` into the `Store` via
//! `apply_agent_event`. Hook position A from the deep dive: after
//! reconcile, before publish — so `poll()` must stay non-blocking.

use ade_core::state::Store;
use ade_core::transcript::TaskId;
use ade_workspace::{Task, WorkspaceProvider};

use super::agent::{AgentBackend, apply_agent_event};

/// Owns the backend and the workspace face for a single [`Task`].
pub struct Supervisor {
    backend: Box<dyn AgentBackend>,
    ws: Box<dyn WorkspaceProvider>,
    task: Task,
}

impl Supervisor {
    pub fn new(task: Task, backend: Box<dyn AgentBackend>, ws: Box<dyn WorkspaceProvider>) -> Self {
        Self { backend, ws, task }
    }

    pub fn task(&self) -> &Task {
        &self.task
    }

    pub fn task_id(&self) -> TaskId {
        self.task.id
    }

    /// Record which opencode session feeds this task (M4c-2 wires the
    /// runtime callsite; the default backend ignores it).
    pub fn bind_session(&mut self, session_id: &str) {
        let id = self.task.id;
        self.backend.bind_session(id, session_id);
    }

    /// Drain new backend events into the store. Idempotent: re-tick with
    /// an empty queue writes nothing.
    pub fn tick(&mut self, store: &mut Store) {
        for ev in self.backend.poll() {
            apply_agent_event(store, &ev);
        }
    }

    pub fn workspace(&mut self) -> &mut dyn WorkspaceProvider {
        &mut *self.ws
    }
}

/// M4 wiring: the runtime drives supervisors through the `ade-core` seam
/// (`drain_fleet` after reconcile, `bind_new_session` on create).
impl ade_core::runtime::fleet::SupervisedTask for Supervisor {
    fn task_id(&self) -> TaskId {
        self.task.id
    }

    fn slug(&self) -> Option<&str> {
        Some(&self.task.slug)
    }

    fn tick(&mut self, store: &mut Store) {
        Supervisor::tick(self, store);
    }

    fn bind_session(&mut self, session_id: &str) {
        Supervisor::bind_session(self, session_id);
    }

    fn unbind_session(&mut self, session_id: &str) {
        self.backend.unbind_session(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::MockAgent;
    use ade_workspace::MockWorkspace;

    fn supervisor_with_prompt(prompt: &str) -> (Supervisor, TaskId) {
        let task = Task::new("feat-x", "mock");
        let id = task.id;
        let mut backend = MockAgent::new();
        backend.spawn(id, prompt).unwrap();
        let sup = Supervisor::new(task, Box::new(backend), Box::new(MockWorkspace::new()));
        (sup, id)
    }

    #[test]
    fn tick_drains_prompt_into_transcript() {
        let (mut sup, id) = supervisor_with_prompt("hello");
        assert_eq!(sup.task_id(), id);
        let mut store = Store::default();
        sup.tick(&mut store);
        let msgs = store.transcripts.get(&id).cloned().unwrap_or_default();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "hello");
        // Re-tick: empty queue writes nothing.
        sup.tick(&mut store);
        assert_eq!(store.transcripts.get(&id).map(|v| v.len()), Some(1));
    }

    #[test]
    fn workspace_face_is_reachable() {
        let (mut sup, _id) = supervisor_with_prompt("hi");
        assert_eq!(sup.workspace().ssh_info(), None);
    }

    #[test]
    fn bind_session_is_safe_for_both_backends() {
        use crate::agent::OpencodeAdapter;

        // MockAgent ignores bindings; tick still drains.
        let (mut sup, id) = supervisor_with_prompt("hello");
        sup.bind_session("s1");
        let mut store = Store::default();
        sup.tick(&mut store);
        assert_eq!(store.transcripts.get(&id).map(|v| v.len()), Some(1));
        // OpencodeAdapter backend: bind + tick stay green, nothing polled
        // (collection goes through `collect_new`, not `poll`).
        let task = Task::new("feat-y", "opencode");
        let mut sup2 = Supervisor::new(
            task,
            Box::new(OpencodeAdapter::new()),
            Box::new(MockWorkspace::new()),
        );
        sup2.bind_session("s1");
        let mut store2 = Store::default();
        sup2.tick(&mut store2);
        assert!(store2.transcripts.is_empty());
    }

    #[test]
    fn seam_trait_delegates_to_inherent_methods() {
        use ade_core::runtime::fleet::SupervisedTask;

        let (mut sup, id) = supervisor_with_prompt("hello");
        // slug() exposes the task slug so the runtime auto-binds on create.
        assert_eq!(SupervisedTask::slug(&sup), Some("feat-x"));
        assert_eq!(SupervisedTask::task_id(&sup), id);
        let mut store = Store::default();
        // Trait tick drains; unbind on a Mock backend is a safe no-op.
        SupervisedTask::tick(&mut sup, &mut store);
        SupervisedTask::bind_session(&mut sup, "s9");
        SupervisedTask::unbind_session(&mut sup, "s9");
        assert_eq!(store.transcripts.get(&id).map(|v| v.len()), Some(1));
    }
}
