//! M3b agent: swappable backend socket (`AgentBackend`).
//!
//! Legacy `server.rs`/`runtime.rs` are untouched; `OpencodeAdapter` only
//! translates between the opencode wire mirror (`Store`) and the unified
//! `AgentEvent` stream. `MockAgent` keeps CI green without a server.

use std::collections::{BTreeMap, VecDeque};

use opencode_codes::protocol_generated::types::SessionStatus;

use base::state::Store;
use base::transcript::{Cost, Role, TaskId, UnifiedMessage};
use base::{UsageSample, now_unix};

#[derive(Debug, Clone, PartialEq)]
pub enum AgentStatus {
    Idle,
    Working,
    NeedsInput { reason: String },
    Done,
    Error { msg: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    Message(UnifiedMessage),
    Status(TaskId, AgentStatus),
    Cost(TaskId, Cost),
    /// Usage observation for the metrics log (M9c). Backends leave
    /// attribution blank when unknown; the `Supervisor` stamps
    /// `agent`/`model` from its `Task` at drain time.
    Usage(UsageSample),
}

/// Agent socket: the agent never knows Host vs VM, only tasks.
pub trait AgentBackend: Send {
    fn spawn(&mut self, task: TaskId, prompt: &str) -> anyhow::Result<()>;
    fn prompt(&mut self, task: &TaskId, text: &str) -> anyhow::Result<()>;
    fn abort(&mut self, task: &TaskId) -> anyhow::Result<()>;
    fn poll(&mut self) -> Vec<AgentEvent>;
    /// Collect new events against a store snapshot. Default: drain `poll()`.
    /// `OpencodeAdapter` overrides this with `collect_new` (cursor dedup).
    fn collect(&mut self, _store: &Store) -> Vec<AgentEvent> {
        self.poll()
    }
    /// Record which session feeds a task. Default: ignore (e.g. `MockAgent`).
    fn bind_session(&mut self, _task: TaskId, _session_id: &str) {}
    /// Forget a retired session. Default: ignore.
    fn unbind_session(&mut self, _session_id: &str) {}
}

/// Scripted backend for tests / CI / no-KVM machines.
#[derive(Debug, Default)]
pub struct MockAgent {
    tasks: BTreeMap<TaskId, AgentStatus>,
    queue: VecDeque<AgentEvent>,
}

impl MockAgent {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status(&self, task: &TaskId) -> Option<&AgentStatus> {
        self.tasks.get(task)
    }

    /// Inject an event as if the agent produced it.
    pub fn emit(&mut self, ev: AgentEvent) {
        match &ev {
            AgentEvent::Status(t, s) => {
                self.tasks.insert(*t, s.clone());
            }
            AgentEvent::Message(m) => {
                self.tasks.entry(m.task).or_insert(AgentStatus::Working);
            }
            AgentEvent::Cost(_, _) | AgentEvent::Usage(_) => {}
        }
        self.queue.push_back(ev);
    }

    fn user_msg(task: TaskId, n: usize, text: &str) -> UnifiedMessage {
        UnifiedMessage {
            id: format!("{task}-u{n}"),
            task,
            role: Role::User,
            text: text.to_string(),
            tool: None,
            ts: 0,
        }
    }
}

impl AgentBackend for MockAgent {
    fn spawn(&mut self, task: TaskId, prompt: &str) -> anyhow::Result<()> {
        self.tasks.insert(task, AgentStatus::Working);
        self.queue
            .push_back(AgentEvent::Status(task, AgentStatus::Working));
        self.queue
            .push_back(AgentEvent::Message(Self::user_msg(task, 0, prompt)));
        Ok(())
    }

    fn prompt(&mut self, task: &TaskId, text: &str) -> anyhow::Result<()> {
        anyhow::ensure!(self.tasks.contains_key(task), "unknown task {task}");
        self.tasks.insert(*task, AgentStatus::Working);
        let n = self.queue.len();
        self.queue
            .push_back(AgentEvent::Status(*task, AgentStatus::Working));
        self.queue
            .push_back(AgentEvent::Message(Self::user_msg(*task, n, text)));
        Ok(())
    }

    fn abort(&mut self, task: &TaskId) -> anyhow::Result<()> {
        anyhow::ensure!(self.tasks.contains_key(task), "unknown task {task}");
        self.tasks.insert(*task, AgentStatus::Idle);
        self.queue
            .push_back(AgentEvent::Status(*task, AgentStatus::Idle));
        Ok(())
    }

    fn poll(&mut self) -> Vec<AgentEvent> {
        self.queue.drain(..).collect()
    }
}

/// Thin translator over the existing opencode mirror. Owns no client:
/// the runtime still drives `server.rs`; the adapter only maps
/// task <-> session and converts `Store` snapshots into `AgentEvent`s.
#[derive(Debug, Default)]
pub struct OpencodeAdapter {
    sessions: BTreeMap<TaskId, String>,
    cursors: BTreeMap<TaskId, usize>,
    /// Last cost forwarded as `Usage` per task (bridge is edge-triggered:
    /// re-polling an unchanged total emits nothing).
    last_cost: BTreeMap<TaskId, Cost>,
}

impl OpencodeAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind(&mut self, task: TaskId, session_id: &str) {
        self.sessions.insert(task, session_id.to_string());
    }
    pub fn unbind(&mut self, task: &TaskId) {
        self.sessions.remove(task);
        self.cursors.remove(task);
        self.last_cost.remove(task);
    }

    pub fn session_of(&self, task: &TaskId) -> Option<&str> {
        self.sessions.get(task).map(String::as_str)
    }

    pub fn status_of(&self, store: &Store, task: &TaskId) -> AgentStatus {
        let Some(sid) = self.sessions.get(task) else {
            return AgentStatus::Error {
                msg: format!("task {task} not bound"),
            };
        };
        if store
            .pending_permissions
            .values()
            .any(|p| &p.session_id == sid)
        {
            return AgentStatus::NeedsInput {
                reason: "permission requested".into(),
            };
        }
        match store.statuses.get(sid) {
            Some(s) => agent_status_of(s),
            None => AgentStatus::Idle,
        }
    }

    /// New transcript messages since the last call, plus a fresh
    /// `Status` per bound task. Idempotent: re-polling emits no dupes.
    /// A changed task total also emits one `Usage` bridge event (M9c);
    /// attribution is blank — the `Supervisor` stamps it at drain time.
    pub fn collect_new(&mut self, store: &Store) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let bound: Vec<(TaskId, String)> =
            self.sessions.iter().map(|(t, s)| (*t, s.clone())).collect();
        for (task, sid) in bound {
            let seen = self.cursors.get(&task).copied().unwrap_or(0);
            let fresh = store.transcript_for_session(&sid);
            for m in fresh.iter().skip(seen) {
                out.push(AgentEvent::Message(m.clone()));
            }
            self.cursors.insert(task, fresh.len());
            if let Some(cost) = store.costs.get(&task)
                && self.last_cost.get(&task) != Some(cost)
            {
                self.last_cost.insert(task, *cost);
                out.push(AgentEvent::Usage(UsageSample::from_cost(
                    task,
                    "",
                    "",
                    *cost,
                    now_unix(),
                )));
            }
            out.push(AgentEvent::Status(task, self.status_of(store, &task)));
        }
        out
    }
}

pub fn agent_status_of(s: &SessionStatus) -> AgentStatus {
    match s {
        SessionStatus::Busy => AgentStatus::Working,
        // Backoff/rate-limit (M9f): not progressing on its own — the same
        // attention class as a permission gate. Attempt + server message
        // ride along so the UI (and M10 notify) can show the wait.
        SessionStatus::Retry {
            attempt, message, ..
        } => AgentStatus::NeedsInput {
            reason: format!(
                "retry #{attempt}: {}",
                message.chars().take(120).collect::<String>()
            ),
        },
        SessionStatus::Idle => AgentStatus::Idle,
    }
}

/// Read-side translator as a backend: session I/O stays with the runtime,
/// so spawn/prompt/abort refuse and `poll` yields nothing — collect via
/// `collect_new(&store)` instead. Only `bind_session` is functional.
impl AgentBackend for OpencodeAdapter {
    fn spawn(&mut self, _task: TaskId, _prompt: &str) -> anyhow::Result<()> {
        anyhow::bail!("OpencodeAdapter is read-side only")
    }

    fn prompt(&mut self, _task: &TaskId, _text: &str) -> anyhow::Result<()> {
        anyhow::bail!("OpencodeAdapter is read-side only")
    }

    fn abort(&mut self, _task: &TaskId) -> anyhow::Result<()> {
        anyhow::bail!("OpencodeAdapter is read-side only")
    }

    fn poll(&mut self) -> Vec<AgentEvent> {
        Vec::new()
    }

    fn collect(&mut self, store: &Store) -> Vec<AgentEvent> {
        self.collect_new(store)
    }

    fn bind_session(&mut self, task: TaskId, session_id: &str) {
        self.bind(task, session_id);
    }

    fn unbind_session(&mut self, session_id: &str) {
        let dead: Vec<TaskId> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.as_str() == session_id)
            .map(|(t, _)| *t)
            .collect();
        for t in dead {
            self.unbind(&t);
        }
    }
}

/// Narrowed applier: the M3d UI path writes transcripts directly from
/// `AgentEvent`s instead of opencode `Event`s. Status is transient
/// (consumer-side); cost replaces the task total (adapter owns accounting).
/// `Usage` is a no-op here: the frozen `Store` gains no metrics field —
/// the `Supervisor` records it into its own `MetricsLog` (M9c).
pub fn apply_agent_event(store: &mut Store, ev: &AgentEvent) {
    match ev {
        AgentEvent::Message(m) => store.push_unified(m.clone()),
        AgentEvent::Cost(task, c) => {
            store.costs.insert(*task, *c);
        }
        AgentEvent::Status(_, _) | AgentEvent::Usage(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_lifecycle() {
        let mut a = MockAgent::new();
        let task = TaskId::new();
        a.spawn(task, "hello").unwrap();
        a.prompt(&task, "again").unwrap();
        a.abort(&task).unwrap();
        let evs = a.poll();
        assert!(evs.contains(&AgentEvent::Status(task, AgentStatus::Working)));
        assert!(evs.contains(&AgentEvent::Status(task, AgentStatus::Idle)));
        assert_eq!(a.status(&task), Some(&AgentStatus::Idle));
        assert!(a.poll().is_empty());
    }

    #[test]
    fn mock_rejects_unknown_task() {
        let mut a = MockAgent::new();
        assert!(a.prompt(&TaskId::new(), "x").is_err());
        assert!(a.abort(&TaskId::new()).is_err());
    }

    #[test]
    fn status_mapping() {
        assert_eq!(agent_status_of(&SessionStatus::Busy), AgentStatus::Working);
        assert_eq!(agent_status_of(&SessionStatus::Idle), AgentStatus::Idle);
        assert!(matches!(
            agent_status_of(&SessionStatus::Retry {
                action: None,
                attempt: 2,
                message: "rate limited, retrying".into(),
                next: 5000,
            }),
            AgentStatus::NeedsInput { .. }
        ));
    }

    #[test]
    fn adapter_collects_only_new() {
        let mut store = Store::default();
        let task = store.task_for_session("s1");
        store.push_unified(UnifiedMessage {
            id: "m1".into(),
            task,
            role: Role::User,
            text: "hi".into(),
            tool: None,
            ts: 0,
        });
        let mut ad = OpencodeAdapter::new();
        ad.bind(task, "s1");
        let first = ad.collect_new(&store);
        assert_eq!(
            first
                .iter()
                .filter(|e| matches!(e, AgentEvent::Message(_)))
                .count(),
            1
        );
        // Re-poll: no duplicate messages, only a fresh Status.
        let second = ad.collect_new(&store);
        assert!(second.iter().all(|e| matches!(e, AgentEvent::Status(_, _))));
        assert_eq!(ad.session_of(&task), Some("s1"));
        ad.unbind(&task);
        assert_eq!(ad.session_of(&task), None);
    }

    fn usage_events(evs: &[AgentEvent]) -> Vec<UsageSample> {
        evs.iter()
            .filter_map(|e| match e {
                AgentEvent::Usage(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn bridge_emits_usage_once_per_cost_change() {
        let mut store = Store::default();
        let task = store.task_for_session("s1");
        let mut ad = OpencodeAdapter::new();
        ad.bind(task, "s1");
        // No cost yet: status only.
        let first = ad.collect_new(&store);
        assert!(first.iter().all(|e| matches!(e, AgentEvent::Status(_, _))));
        // Cost appears: one Usage bridge (+ status), attribution blank.
        store.costs.insert(
            task,
            Cost {
                input: 5.0,
                output: 1.0,
                cache: 0.0,
                cost: 0.1,
            },
        );
        let second = ad.collect_new(&store);
        let usages = usage_events(&second);
        assert_eq!(usages.len(), 1);
        assert_eq!(usages[0].task, task);
        assert_eq!(usages[0].cost, Some(0.1));
        assert!(usages[0].agent.is_empty());
        // Unchanged total: edge-triggered, no second Usage.
        let third = ad.collect_new(&store);
        assert!(usage_events(&third).is_empty());
        // Changed total: emits again.
        store.costs.insert(
            task,
            Cost {
                input: 6.0,
                output: 1.0,
                cache: 0.0,
                cost: 0.2,
            },
        );
        let fourth = ad.collect_new(&store);
        assert_eq!(usage_events(&fourth).len(), 1);
        // Unbind clears the edge memory: rebind re-emits once.
        ad.unbind(&task);
        ad.bind(task, "s1");
        let fifth = ad.collect_new(&store);
        assert_eq!(usage_events(&fifth).len(), 1);
    }

    #[test]
    fn apply_agent_event_writes_transcript_and_cost() {
        let mut store = Store::default();
        let task = TaskId::new();
        let msg = UnifiedMessage {
            id: "m1".into(),
            task,
            role: Role::Agent,
            text: "done".into(),
            tool: None,
            ts: 1,
        };
        apply_agent_event(&mut store, &AgentEvent::Message(msg));
        apply_agent_event(
            &mut store,
            &AgentEvent::Cost(
                task,
                Cost {
                    input: 5.0,
                    output: 0.0,
                    cache: 0.0,
                    cost: 0.1,
                },
            ),
        );
        assert_eq!(store.transcripts.get(&task).unwrap().len(), 1);
        assert_eq!(store.costs.get(&task).unwrap().input, 5.0);
    }

    #[test]
    fn adapter_session_binding_roundtrips_through_trait() {
        let task = TaskId::new();
        let mut ad = OpencodeAdapter::new();
        {
            let backend: &mut dyn AgentBackend = &mut ad;
            // Read-side backend refuses session I/O but binds fine.
            assert!(backend.spawn(task, "x").is_err());
            backend.bind_session(task, "s7");
            assert!(backend.poll().is_empty());
        }
        assert_eq!(ad.session_of(&task), Some("s7"));
        {
            let backend: &mut dyn AgentBackend = &mut ad;
            backend.unbind_session("s7");
            // Unknown session unbind is a no-op.
            backend.unbind_session("nope");
        }
        assert_eq!(ad.session_of(&task), None);
    }

    #[test]
    fn collect_reports_needs_input_on_permission() {
        use base::PendingPermission;

        let mut store = Store::default();
        let task = store.task_for_session("s1");
        store.pending_permissions.insert(
            "p1".into(),
            PendingPermission {
                permission_id: "p1".into(),
                session_id: "s1".into(),
                ..Default::default()
            },
        );
        let mut ad = OpencodeAdapter::new();
        ad.bind(task, "s1");
        let evs = ad.collect(&store);
        assert!(
            evs.iter().any(|e| matches!(
                e,
                AgentEvent::Status(t, AgentStatus::NeedsInput { .. }) if *t == task
            )),
            "expected NeedsInput, got {evs:?}"
        );
    }
}
