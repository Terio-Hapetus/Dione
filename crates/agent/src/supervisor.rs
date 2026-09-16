//! M4c supervisor: one task = one backend + one workspace face.
//!
//! Legacy `runtime.rs` is untouched; the runtime (M4d dispatcher) drives
//! `tick`, which only drains `backend.poll()` into the `Store` via
//! `apply_agent_event`. Hook position A from the deep dive: after
//! reconcile, before publish — so `poll()` must stay non-blocking.

use base::state::Store;
use base::transcript::TaskId;
use workspace::{Task, TaskStatus, WorkspaceProvider, handoff_summary};

use super::agent::{AgentBackend, AgentEvent, AgentStatus, apply_agent_event};

/// Owns the backend and the workspace face for a single [`Task`].
pub struct Supervisor {
    backend: Box<dyn AgentBackend>,
    ws: Box<dyn WorkspaceProvider>,
    task: Task,
    /// Backend `Error` hits since the last inbox drain (M7a retry budget).
    error_hits: Vec<TaskId>,
    /// Backend good-terminal hits (`Idle`/`Done`) since the last drain
    /// (fix-2 success reset, mirrors `error_hits`).
    success_hits: Vec<TaskId>,
}

impl Supervisor {
    pub fn new(task: Task, backend: Box<dyn AgentBackend>, ws: Box<dyn WorkspaceProvider>) -> Self {
        Self {
            backend,
            ws,
            task,
            error_hits: Vec::new(),
            success_hits: Vec::new(),
        }
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
    /// an empty queue writes nothing. Uses `collect` (not `poll`) so
    /// read-side backends (`OpencodeAdapter`) contribute their cursor
    /// stream too. `Error` statuses are also recorded for the M7a retry
    /// budget (forwarded via `drain_errors`); successes clear the streak.
    /// On the transition to `Blocked`, a handoff summary is snapshotted
    /// from the transcript tail unless one was set by hand (M7c).
    pub fn tick(&mut self, store: &mut Store) {
        for ev in self.backend.collect(store) {
            match &ev {
                AgentEvent::Status(id, AgentStatus::Error { .. }) => {
                    self.error_hits.push(*id);
                    if self.task.note_failure() == TaskStatus::Blocked
                        && self.task.summary.is_none()
                    {
                        let id = self.task.id;
                        self.task.summary = store
                            .transcripts
                            .get(&id)
                            .and_then(|msgs| handoff_summary(msgs, 3, 500));
                    }
                }
                // Success resets the streak — except on a Blocked task,
                // which must not silently un-block the sweeper side.
                AgentEvent::Status(_, AgentStatus::Idle)
                | AgentEvent::Status(_, AgentStatus::Done)
                    if self.task.status != TaskStatus::Blocked =>
                {
                    self.task.note_success();
                    self.success_hits.push(self.task.id);
                }
                _ => {}
            }
            apply_agent_event(store, &ev);
        }
    }

    /// Backend error hits since the last call; drains the buffer.
    pub fn drain_errors(&mut self) -> Vec<TaskId> {
        std::mem::take(&mut self.error_hits)
    }

    /// Backend success hits since the last call; drains the buffer.
    pub fn drain_successes(&mut self) -> Vec<TaskId> {
        std::mem::take(&mut self.success_hits)
    }

    /// Manual retry (M7b): budget restored, error buffer cleared.
    pub fn retry_reset(&mut self) {
        self.task.retry_reset();
        self.error_hits.clear();
        self.success_hits.clear();
    }

    pub fn workspace(&mut self) -> &mut dyn WorkspaceProvider {
        &mut *self.ws
    }
}

/// M4 wiring: the runtime drives supervisors through the `base` seam
/// (`drain_fleet` after reconcile, `bind_new_session` on create).
impl base::runtime::fleet::SupervisedTask for Supervisor {
    fn task_id(&self) -> TaskId {
        self.task.id
    }

    fn slug(&self) -> Option<&str> {
        Some(&self.task.slug)
    }

    fn failure_limit(&self) -> u8 {
        self.task.failure_limit
    }

    fn max_runtime_secs(&self) -> Option<u64> {
        self.task.max_runtime_secs
    }

    fn tick(&mut self, store: &mut Store) {
        Supervisor::tick(self, store);
    }

    fn drain_errors(&mut self) -> Vec<TaskId> {
        Supervisor::drain_errors(self)
    }

    fn drain_successes(&mut self) -> Vec<TaskId> {
        Supervisor::drain_successes(self)
    }

    fn retry_reset(&mut self) {
        Supervisor::retry_reset(self)
    }

    fn handoff(&self) -> Option<base::runtime::fleet::TaskHandoff> {
        Some(base::runtime::fleet::TaskHandoff {
            parent: self.task.id,
            slug: self.task.slug.clone(),
            agent_ref: self.task.agent_ref.clone(),
            summary: self.task.summary.clone(),
        })
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
    use workspace::MockWorkspace;

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
        // MockWorkspace has no canned output: exec errs instead of hanging.
        assert!(
            sup.workspace()
                .exec(&["true"], std::path::Path::new("/"))
                .is_err()
        );
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
        use base::runtime::fleet::SupervisedTask;

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

    #[test]
    fn adapter_backend_collects_through_tick_without_dupes() {
        use crate::agent::OpencodeAdapter;
        use base::transcript::{Role, UnifiedMessage};

        let task = Task::new("feat-z", "opencode");
        let mut sup = Supervisor::new(
            task,
            Box::new(OpencodeAdapter::new()),
            Box::new(MockWorkspace::new()),
        );
        let mut store = Store::default();
        let sid_task = store.task_for_session("s1");
        store.push_unified(UnifiedMessage {
            id: "m1".into(),
            task: sid_task,
            role: Role::Agent,
            text: "hi".into(),
            tool: None,
            ts: 0,
        });
        sup.bind_session("s1");
        sup.tick(&mut store);
        assert_eq!(store.transcripts.get(&sid_task).map(|v| v.len()), Some(1));
        // Second tick: cursor dedup, no duplicate messages.
        sup.tick(&mut store);
        assert_eq!(store.transcripts.get(&sid_task).map(|v| v.len()), Some(1));
    }

    #[test]
    fn error_events_feed_retry_budget_and_drain() {
        use crate::agent::{AgentEvent, AgentStatus};
        use workspace::TaskStatus;

        // Fresh supervisor: spawn drains with no errors, stays Active.
        let (mut sup, _id) = supervisor_with_prompt("hello");
        let mut store = Store::default();
        sup.tick(&mut store);
        assert!(sup.drain_errors().is_empty());
        assert_eq!(sup.task().status, TaskStatus::Active);

        // Two injected errors hit the budget (limit 2) and block the task.
        let task2 = Task::new("feat-e2", "mock").with_failure_limit(2);
        let id2 = task2.id;
        let mut backend2 = MockAgent::new();
        backend2.emit(AgentEvent::Status(
            id2,
            AgentStatus::Error { msg: "boom".into() },
        ));
        backend2.emit(AgentEvent::Status(
            id2,
            AgentStatus::Error {
                msg: "boom2".into(),
            },
        ));
        let mut sup2 = Supervisor::new(task2, Box::new(backend2), Box::new(MockWorkspace::new()));
        sup2.tick(&mut store);
        assert_eq!(sup2.drain_errors(), vec![id2, id2]);
        assert_eq!(sup2.task().status, TaskStatus::Blocked);
        // Drained buffer stays empty.
        assert!(sup2.drain_errors().is_empty());

        // One error under a limit of 3 is Retrying; success clears it.
        let task3 = Task::new("feat-e3", "mock").with_failure_limit(3);
        let id3 = task3.id;
        let mut backend3 = MockAgent::new();
        backend3.emit(AgentEvent::Status(
            id3,
            AgentStatus::Error {
                msg: "flaky".into(),
            },
        ));
        let mut sup3 = Supervisor::new(task3, Box::new(backend3), Box::new(MockWorkspace::new()));
        sup3.tick(&mut store);
        assert_eq!(sup3.drain_errors(), vec![id3]);
        assert_eq!(sup3.task().status, TaskStatus::Retrying);
    }

    #[test]
    fn seam_reports_task_failure_limit() {
        use base::runtime::fleet::SupervisedTask;

        let (sup, _) = supervisor_with_prompt("hello");
        assert_eq!(SupervisedTask::failure_limit(&sup), 2);
        let task = Task::new("feat-lim", "mock").with_failure_limit(4);
        let lim = Supervisor::new(
            task,
            Box::new(MockAgent::new()),
            Box::new(MockWorkspace::new()),
        );
        assert_eq!(SupervisedTask::failure_limit(&lim), 4);
    }

    #[test]
    fn inbox_chain_blocks_erroring_task_on_sweep_tick() {
        use crate::agent::{AgentEvent, AgentStatus};
        use crate::sweeper::FleetSweeper;
        use base::runtime::FleetInbox;
        use workspace::Task;

        let task = Task::new("wt-err", "mock"); // default limit 2
        let id = task.id;
        let mut backend = MockAgent::new();
        backend.emit(AgentEvent::Status(
            id,
            AgentStatus::Error { msg: "x".into() },
        ));
        backend.emit(AgentEvent::Status(
            id,
            AgentStatus::Error { msg: "y".into() },
        ));
        let inbox = FleetInbox::new();
        inbox.register_sweeper(Box::new(FleetSweeper::new()));
        inbox.register_task(Box::new(Supervisor::new(
            task,
            Box::new(backend),
            Box::new(MockWorkspace::new()),
        )));
        inbox.bind_new("wt-err", "s1");
        let mut store = Store::default();
        // Tick 1 drains backend errors into the sweeper; nothing surfaces yet.
        inbox.poll_fleet(&mut store, 1);
        assert!(store.errors.is_empty());
        // Tick 30 runs the sweep: 2 errors hit the default budget → blocked.
        inbox.poll_fleet(&mut store, 30);
        assert_eq!(store.errors.len(), 1);
        assert!(store.errors[0].contains("blocked"));
        assert!(store.is_blocked_slug("wt-err"));
        // Next sweep stays silent: blocked tasks are untracked.
        inbox.poll_fleet(&mut store, 60);
        assert_eq!(store.errors.len(), 1);
        // Manual retry re-tracks fresh; the sweep stays silent with no
        // new errors. (The handler clears the Store mirror separately.)
        assert!(inbox.retry_task("wt-err"));
        assert!(store.is_blocked_slug("wt-err"));
        store.clear_blocked_by_slug("wt-err");
        inbox.poll_fleet(&mut store, 90);
        assert_eq!(store.errors.len(), 1);
        assert!(!inbox.retry_task("wt-unknown"));
    }

    #[test]
    fn supervisor_retry_reset_restores_task() {
        use crate::agent::{AgentEvent, AgentStatus};
        use workspace::TaskStatus;

        let task = Task::new("feat-r", "mock").with_failure_limit(1);
        let id = task.id;
        let mut backend = MockAgent::new();
        backend.emit(AgentEvent::Status(
            id,
            AgentStatus::Error { msg: "x".into() },
        ));
        let mut sup = Supervisor::new(task, Box::new(backend), Box::new(MockWorkspace::new()));
        let mut store = Store::default();
        sup.tick(&mut store);
        assert_eq!(sup.task().status, TaskStatus::Blocked);
        sup.retry_reset();
        assert_eq!(sup.task().status, TaskStatus::Active);
        assert!(sup.drain_errors().is_empty());
    }

    #[test]
    fn block_snapshots_handoff_summary_unless_handwritten() {
        use crate::agent::{AgentEvent, AgentStatus};
        use base::transcript::{Role, UnifiedMessage};

        fn seed(store: &mut Store, id: TaskId, texts: &[&str]) {
            for (n, t) in texts.iter().enumerate() {
                store.push_unified(UnifiedMessage {
                    id: format!("m{n}"),
                    task: id,
                    role: Role::Agent,
                    text: (*t).into(),
                    tool: None,
                    ts: 0,
                });
            }
        }

        // Auto-snapshot from the transcript tail on Blocked.
        let task = Task::new("feat-h", "mock").with_failure_limit(1);
        let id = task.id;
        let mut store = Store::default();
        seed(&mut store, id, &["tried oauth", "stuck on refresh"]);
        let mut backend = MockAgent::new();
        backend.emit(AgentEvent::Status(
            id,
            AgentStatus::Error { msg: "x".into() },
        ));
        let mut sup = Supervisor::new(task, Box::new(backend), Box::new(MockWorkspace::new()));
        sup.tick(&mut store);
        assert_eq!(
            sup.task().summary.as_deref(),
            Some("agent: tried oauth\nagent: stuck on refresh")
        );

        // A handwritten summary is never overwritten.
        let task2 = Task::new("feat-h2", "mock")
            .with_failure_limit(1)
            .with_summary("human note");
        let id2 = task2.id;
        seed(&mut store, id2, &["other work"]);
        let mut backend2 = MockAgent::new();
        backend2.emit(AgentEvent::Status(
            id2,
            AgentStatus::Error { msg: "y".into() },
        ));
        let mut sup2 = Supervisor::new(task2, Box::new(backend2), Box::new(MockWorkspace::new()));
        sup2.tick(&mut store);
        assert_eq!(sup2.task().summary.as_deref(), Some("human note"));
    }

    #[test]
    fn seam_handoff_snapshot() {
        use base::runtime::fleet::{SupervisedTask, TaskHandoff};

        let (sup, id) = supervisor_with_prompt("hello");
        assert_eq!(
            SupervisedTask::handoff(&sup),
            Some(TaskHandoff {
                parent: id,
                slug: "feat-x".into(),
                agent_ref: "mock".into(),
                summary: None,
            })
        );
    }

    #[test]
    fn idle_reports_success_hits_but_never_unblocks() {
        use crate::agent::{AgentEvent, AgentStatus};
        use workspace::TaskStatus;

        // Idle on a healthy task records one success hit.
        let (mut sup, id) = supervisor_with_prompt("hello");
        let mut store = Store::default();
        sup.tick(&mut store);
        assert!(sup.drain_successes().is_empty());
        let mut backend = MockAgent::new();
        backend.emit(AgentEvent::Status(id, AgentStatus::Idle));
        // Swap backend via a fresh supervisor on the same task id.
        let task = sup.task().clone();
        let mut sup2 = Supervisor::new(task, Box::new(backend), Box::new(MockWorkspace::new()));
        sup2.tick(&mut store);
        assert_eq!(sup2.drain_successes(), vec![id]);
        assert_eq!(sup2.task().status, TaskStatus::Active);
        // Idle on a Blocked task reports nothing (no silent un-block).
        let mut btask = Task::new("feat-b", "mock").with_failure_limit(1);
        btask.note_failure();
        assert_eq!(btask.status, TaskStatus::Blocked);
        let mut bbackend = MockAgent::new();
        bbackend.emit(AgentEvent::Status(btask.id, AgentStatus::Idle));
        let mut bsup = Supervisor::new(btask, Box::new(bbackend), Box::new(MockWorkspace::new()));
        bsup.tick(&mut store);
        assert!(bsup.drain_successes().is_empty());
        assert_eq!(bsup.task().status, TaskStatus::Blocked);
    }

    #[test]
    fn inbox_chain_error_success_error_stays_quiet() {
        use crate::agent::{AgentEvent, AgentStatus};
        use crate::sweeper::FleetSweeper;
        use base::runtime::FleetInbox;
        use workspace::Task;

        // Limit 2: error, success, error must not block at the sweep.
        let task = Task::new("wt-mix", "mock");
        let id = task.id;
        let mut backend = MockAgent::new();
        backend.emit(AgentEvent::Status(
            id,
            AgentStatus::Error { msg: "x".into() },
        ));
        backend.emit(AgentEvent::Status(id, AgentStatus::Idle));
        backend.emit(AgentEvent::Status(
            id,
            AgentStatus::Error { msg: "y".into() },
        ));
        let inbox = FleetInbox::new();
        inbox.register_sweeper(Box::new(FleetSweeper::new()));
        inbox.register_task(Box::new(Supervisor::new(
            task,
            Box::new(backend),
            Box::new(MockWorkspace::new()),
        )));
        inbox.bind_new("wt-mix", "s1");
        let mut store = Store::default();
        inbox.poll_fleet(&mut store, 1);
        inbox.poll_fleet(&mut store, 30);
        assert!(store.errors.is_empty(), "unexpected: {:?}", store.errors);
        assert!(!store.is_blocked_slug("wt-mix"));
    }
}
