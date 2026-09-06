//! M3b agent: swappable backend socket (`AgentBackend`).
//!
//! Legacy `server.rs`/`runtime.rs` are untouched; `OpencodeAdapter` only
//! translates between the opencode wire mirror (`Store`) and the unified
//! `AgentEvent` stream. `MockAgent` keeps CI green without a server.

use std::collections::{BTreeMap, VecDeque};

use opencode_codes::protocol_generated::types::SessionStatus;

use crate::state::Store;
use crate::transcript::{Cost, Role, TaskId, UnifiedMessage};

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
}

/// Agent socket: the agent never knows Host vs VM, only tasks.
pub trait AgentBackend: Send {
    fn spawn(&mut self, task: TaskId, prompt: &str) -> anyhow::Result<()>;
    fn prompt(&mut self, task: &TaskId, text: &str) -> anyhow::Result<()>;
    fn abort(&mut self, task: &TaskId) -> anyhow::Result<()>;
    fn poll(&mut self) -> Vec<AgentEvent>;
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
            AgentEvent::Cost(_, _) => {}
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
            out.push(AgentEvent::Status(task, self.status_of(store, &task)));
        }
        out
    }
}

pub fn agent_status_of(s: &SessionStatus) -> AgentStatus {
    match s {
        SessionStatus::Busy | SessionStatus::Retry { .. } => AgentStatus::Working,
        SessionStatus::Idle => AgentStatus::Idle,
    }
}

/// Narrowed applier: the M3d UI path writes transcripts directly from
/// `AgentEvent`s instead of opencode `Event`s. Status is transient
/// (consumer-side); cost replaces the task total (adapter owns accounting).
pub fn apply_agent_event(store: &mut Store, ev: &AgentEvent) {
    match ev {
        AgentEvent::Message(m) => store.push_unified(m.clone()),
        AgentEvent::Cost(task, c) => {
            store.costs.insert(*task, *c);
        }
        AgentEvent::Status(_, _) => {}
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
}
