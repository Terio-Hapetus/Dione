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
    /// Drain new backend events into the store. Must be non-blocking:
    /// it runs inside the runtime `select!` loop.
    fn tick(&mut self, store: &mut Store);
    /// Record which session feeds this task. Default: ignore.
    fn bind_session(&mut self, _session_id: &str) {}
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

/// Periodic triage over tracked sessions. Sessions (not tasks) are the
/// currency here so `ade-core` never names workspace types.
pub trait TaskSweeper: Send {
    fn track_session(&mut self, session_id: &str);
    fn untrack_session(&mut self, session_id: &str);
    fn note_session_error(&mut self, session_id: &str);
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

/// Minimal action consumer (slice 4 refines into retry/banner): surface
/// both actions as errors so nothing is silently dropped.
pub(crate) fn apply_sweep(store: &mut Store, actions: &[SweepAction]) {
    for a in actions {
        match a {
            SweepAction::Reclaim(id) => store.push_error(format!("fleet: reclaim {id}")),
            SweepAction::Blocked(id) => store.push_error(format!("fleet: blocked {id}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Role, UnifiedMessage};

    struct StubTask {
        id: TaskId,
        ticks: usize,
        bound: Vec<String>,
    }

    impl StubTask {
        fn new() -> Self {
            Self {
                id: TaskId::new(),
                ticks: 0,
                bound: Vec::new(),
            }
        }
    }

    impl SupervisedTask for StubTask {
        fn task_id(&self) -> TaskId {
            self.id
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

        fn bind_session(&mut self, session_id: &str) {
            self.bound.push(session_id.to_string());
        }
    }

    struct StubSweeper {
        actions: Vec<SweepAction>,
    }

    impl TaskSweeper for StubSweeper {
        fn track_session(&mut self, _session_id: &str) {}
        fn untrack_session(&mut self, _session_id: &str) {}
        fn note_session_error(&mut self, _session_id: &str) {}
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
}
