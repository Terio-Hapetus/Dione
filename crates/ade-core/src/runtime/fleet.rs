//! M4 fleet seam: supervisor/dispatcher hooks without reverse deps.
//!
//! `ade-core` owns the tick cadence; `ade-agent` (`Supervisor`) and
//! `ade-workspace` (`Dispatcher`) plug in concrete hooks behind these
//! traits. That keeps the one-way chain `agent → workspace → core`:
//! nothing here imports either crate.

use crate::state::Store;
use crate::transcript::TaskId;

/// One supervised unit (task + backend + workspace face), backend-agnostic.
/// Implemented by `ade-agent::Supervisor`.
pub trait SupervisedTask: Send {
    fn task_id(&self) -> TaskId;
    /// Worktree slug this task owns (`1 task = 1 worktree` convention).
    /// `None` = never auto-bound to new sessions.
    fn slug(&self) -> Option<&str> {
        None
    }
    /// Drain new backend events into the store. Must be non-blocking:
    /// it runs inside the runtime `select!` loop.
    fn tick(&mut self, store: &mut Store);
    /// Record which session feeds this task. Default: ignore.
    fn bind_session(&mut self, _session_id: &str) {}
    /// Forget a retired session. Default: ignore.
    fn unbind_session(&mut self, _session_id: &str) {}
}

/// Kanban-lite sweep report for one task. Pure report, no side effects.
/// Produced by `ade-workspace::Dispatcher` via an adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepAction {
    /// Overdue or stale: take the task back for reassignment/retry.
    Reclaim(TaskId),
    /// Errored twice in a row: stop auto-retry, needs a human look.
    Blocked(TaskId),
}

/// Periodic triage over tracked tasks. Tasks (not sessions) are the
/// currency so the sweeper never names workspace types; `TaskId` is
/// `ade-core`'s own transcript key.
pub trait TaskSweeper: Send {
    fn track_task(&mut self, task: TaskId);
    fn untrack_task(&mut self, task: &TaskId);
    fn note_task_error(&mut self, task: &TaskId);
    fn sweep(&self) -> Vec<SweepAction>;
}

/// Poll ticks per sweep: 60s sweep over a 2s poll (matches the providers
/// pattern of `tick.is_multiple_of(10)` for a 20s cadence).
pub(crate) fn sweep_due(tick: u64) -> bool {
    tick.is_multiple_of(30)
}

/// Drain every supervised task into the store. Empty fleet = no-op.
pub(crate) fn drain_fleet(fleet: &mut [Box<dyn SupervisedTask>], store: &mut Store) {
    for t in fleet.iter_mut() {
        t.tick(store);
    }
}

/// Action consumer: surface both actions as errors so nothing is
/// silently dropped. Reclaim hints retry; Blocked asks for a human.
pub fn apply_sweep(store: &mut Store, actions: &[SweepAction]) {
    for a in actions {
        match a {
            SweepAction::Reclaim(id) => {
                store.push_error(format!("fleet: reclaim {id} (overdue/stale, will retry)"));
            }
            SweepAction::Blocked(id) => {
                store.push_error(format!(
                    "fleet: blocked {id} (2 errors, needs a human look)"
                ));
            }
        }
    }
}

/// Bind a fresh session to the supervised tasks owning its scope and
/// track those tasks for sweep. Root scope ("") is never task-bound.
pub(crate) fn bind_new_session(
    fleet: &mut [Box<dyn SupervisedTask>],
    sweeper: &mut Option<Box<dyn TaskSweeper>>,
    scope: &str,
    session_id: &str,
) {
    if scope.is_empty() {
        return;
    }
    for t in fleet.iter_mut() {
        if t.slug().is_some_and(|sl| sl == scope) {
            t.bind_session(session_id);
            if let Some(sw) = sweeper.as_mut() {
                sw.track_task(t.task_id());
            }
        }
    }
}

/// Forget retired sessions: unbind every task, untrack their tasks.
/// Call before `retire_session` drops the `session_task` map.
pub(crate) fn release_sessions(
    fleet: &mut [Box<dyn SupervisedTask>],
    sweeper: &mut Option<Box<dyn TaskSweeper>>,
    store: &Store,
    sids: &[String],
) {
    for sid in sids {
        for t in fleet.iter_mut() {
            t.unbind_session(sid);
        }
        if let Some(task) = store.session_task.get(sid)
            && let Some(sw) = sweeper.as_mut()
        {
            sw.untrack_task(task);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Role, UnifiedMessage};

    struct StubTask {
        id: TaskId,
        owned_slug: Option<String>,
        ticks: usize,
    }

    impl StubTask {
        fn new() -> Self {
            Self {
                id: TaskId::new(),
                owned_slug: None,
                ticks: 0,
            }
        }
    }

    impl SupervisedTask for StubTask {
        fn task_id(&self) -> TaskId {
            self.id
        }

        fn slug(&self) -> Option<&str> {
            self.owned_slug.as_deref()
        }

        fn tick(&mut self, store: &mut Store) {
            self.ticks += 1;
            store.push_unified(UnifiedMessage {
                id: format!("{}-{}", self.id, self.ticks),
                task: self.id,
                role: Role::Agent,
                text: "tick".into(),
                tool: None,
                ts: 0,
            });
        }

        fn bind_session(&mut self, _session_id: &str) {}

        fn unbind_session(&mut self, _session_id: &str) {}
    }

    struct StubSweeper {
        actions: Vec<SweepAction>,
    }

    impl TaskSweeper for StubSweeper {
        fn track_task(&mut self, _task: TaskId) {}
        fn untrack_task(&mut self, _task: &TaskId) {}
        fn note_task_error(&mut self, _task: &TaskId) {}
        fn sweep(&self) -> Vec<SweepAction> {
            self.actions.clone()
        }
    }

    #[test]
    fn sweep_cadence_is_60s_over_2s_poll() {
        assert!(!sweep_due(1));
        assert!(!sweep_due(29));
        assert!(sweep_due(30));
        assert!(sweep_due(60));
    }

    #[test]
    fn drain_fleet_ticks_every_task() {
        let mut store = Store::default();
        let mut fleet: Vec<Box<dyn SupervisedTask>> =
            vec![Box::new(StubTask::new()), Box::new(StubTask::new())];
        drain_fleet(&mut fleet, &mut store);
        assert_eq!(store.transcripts.len(), 2);
        // Empty fleet writes nothing.
        drain_fleet(&mut [], &mut store);
        assert_eq!(store.transcripts.len(), 2);
    }

    #[test]
    fn apply_sweep_surfaces_both_actions() {
        let mut store = Store::default();
        let sw = StubSweeper { actions: vec![] };
        apply_sweep(&mut store, &sw.sweep());
        assert!(store.errors.is_empty());
        let id = TaskId::new();
        apply_sweep(
            &mut store,
            &[SweepAction::Reclaim(id), SweepAction::Blocked(id)],
        );
        assert_eq!(store.errors.len(), 2);
    }

    use std::sync::{Arc, Mutex};

    /// Local stub sharing an append-only log so tests can observe
    /// behind-trait-object calls.
    struct Logged {
        id: TaskId,
        owned_slug: Option<String>,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl SupervisedTask for Logged {
        fn task_id(&self) -> TaskId {
            self.id
        }

        fn slug(&self) -> Option<&str> {
            self.owned_slug.as_deref()
        }

        fn tick(&mut self, _store: &mut Store) {}

        fn bind_session(&mut self, sid: &str) {
            self.log.lock().unwrap().push(format!("bind:{sid}"));
        }

        fn unbind_session(&mut self, sid: &str) {
            self.log.lock().unwrap().push(format!("unbind:{sid}"));
        }
    }

    struct LoggedSweeper {
        log: Arc<Mutex<Vec<String>>>,
    }

    impl TaskSweeper for LoggedSweeper {
        fn track_task(&mut self, task: TaskId) {
            self.log.lock().unwrap().push(format!("track:{task}"));
        }
        fn untrack_task(&mut self, task: &TaskId) {
            self.log.lock().unwrap().push(format!("untrack:{task}"));
        }
        fn note_task_error(&mut self, _task: &TaskId) {}
        fn sweep(&self) -> Vec<SweepAction> {
            Vec::new()
        }
    }

    #[test]
    fn bind_new_session_matches_slug_only() {
        let log: Arc<Mutex<Vec<String>>> = Default::default();
        let mk = |slug: Option<&str>| Logged {
            id: TaskId::new(),
            owned_slug: slug.map(str::to_string),
            log: Arc::clone(&log),
        };
        let mut fleet: Vec<Box<dyn SupervisedTask>> =
            vec![Box::new(mk(Some("wt-a"))), Box::new(mk(None))];
        let mut sweeper: Option<Box<dyn TaskSweeper>> = Some(Box::new(LoggedSweeper {
            log: Arc::clone(&log),
        }));
        bind_new_session(&mut fleet, &mut sweeper, "wt-a", "s1");
        // Root scope never binds, even with matching tasks present.
        bind_new_session(&mut fleet, &mut sweeper, "", "s-root");
        let got = log.lock().unwrap().clone();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], "bind:s1");
        assert!(got[1].starts_with("track:"));
    }

    #[test]
    fn release_sessions_unbinds_and_untracks() {
        let mut store = Store::default();
        let task = store.task_for_session("s1");
        let log: Arc<Mutex<Vec<String>>> = Default::default();
        let mut fleet: Vec<Box<dyn SupervisedTask>> = vec![Box::new(Logged {
            id: task,
            owned_slug: Some("wt-a".into()),
            log: Arc::clone(&log),
        })];
        let mut sweeper: Option<Box<dyn TaskSweeper>> = Some(Box::new(LoggedSweeper {
            log: Arc::clone(&log),
        }));
        release_sessions(&mut fleet, &mut sweeper, &store, &["s1".to_string()]);
        // Releasing an unknown session still unbinds, never crashes.
        release_sessions(&mut fleet, &mut sweeper, &store, &["nope".to_string()]);
        let got = log.lock().unwrap().clone();
        assert_eq!(
            got,
            vec![
                "unbind:s1".to_string(),
                format!("untrack:{task}"),
                "unbind:nope".to_string(),
            ]
        );
    }
}
