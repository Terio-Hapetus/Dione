//! M4 sweeper adapter: `Dispatcher` behind the `base` seam.
//!
//! The runtime owns the 60s cadence (`sweep_due`) and the consumer
//! (`apply_sweep`); this adapter only translates between the two
//! vocabularies so neither crate names the other's types.

use base::runtime::fleet::{SweepAction, TaskSweeper};
use base::transcript::TaskId;
use workspace::{Dispatcher, DispatcherAction};

/// `workspace::Dispatcher` as an `base::TaskSweeper`.
#[derive(Debug)]
pub struct FleetSweeper {
    inner: Dispatcher,
}

impl FleetSweeper {
    pub fn new() -> Self {
        // NOTE: `Dispatcher::default()` would zero the sweep interval and
        // reclaim every fresh task; `new()` keeps the 60s cadence.
        Self {
            inner: Dispatcher::new(),
        }
    }
}

impl Default for FleetSweeper {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskSweeper for FleetSweeper {
    fn track_task(&mut self, task: TaskId) {
        self.inner.track_id(task);
    }

    fn track_task_with_limit(&mut self, task: TaskId, limit: u8) {
        self.inner.track_id_with_limit(task, limit);
    }

    fn track_task_full(&mut self, task: TaskId, limit: u8, max_runtime_secs: Option<u64>) {
        self.inner.track_full(task, limit, max_runtime_secs);
    }

    fn untrack_task(&mut self, task: &TaskId) {
        self.inner.untrack(task);
    }

    fn note_task_error(&mut self, task: &TaskId) {
        self.inner.note_error(task);
    }

    fn note_task_success(&mut self, task: &TaskId) {
        self.inner.note_success(task);
    }

    fn note_heartbeat(&mut self, task: &TaskId) {
        self.inner.note_heartbeat(task);
    }

    fn sweep(&self) -> Vec<SweepAction> {
        self.inner
            .sweep()
            .into_iter()
            .map(|a| match a {
                DispatcherAction::Reclaim(id) => SweepAction::Reclaim(id),
                DispatcherAction::Blocked(id) => SweepAction::Blocked(id),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base::runtime::fleet::apply_sweep;
    use base::state::Store;

    #[test]
    fn chain_tracks_sweeps_and_applies() {
        let mut sw = FleetSweeper::new();
        let id = TaskId::new();
        // Fresh: silent.
        sw.track_task(id);
        assert!(sw.sweep().is_empty());
        // Two errors: blocked, surfaced as an error entry.
        sw.note_task_error(&id);
        sw.note_task_error(&id);
        let actions = sw.sweep();
        assert_eq!(actions, vec![SweepAction::Blocked(id)]);
        let mut store = Store::default();
        apply_sweep(&mut store, &actions);
        assert_eq!(store.errors.len(), 1);
        assert!(store.errors[0].contains("blocked"));
        // Untracked: silent again.
        sw.untrack_task(&id);
        assert!(sw.sweep().is_empty());
    }

    #[test]
    fn reclaim_surfaces_retry_hint() {
        use base::runtime::fleet::SweepAction;

        let mut store = Store::default();
        apply_sweep(&mut store, &[SweepAction::Reclaim(TaskId::new())]);
        assert!(store.errors[0].contains("reclaim"));
    }

    #[test]
    fn success_resets_streak_through_adapter() {
        let mut sw = FleetSweeper::new();
        let id = TaskId::new();
        sw.track_task_with_limit(id, 2);
        sw.note_task_error(&id);
        sw.note_task_success(&id);
        sw.note_task_error(&id);
        assert!(
            sw.sweep().is_empty(),
            "error→success→error stays under budget"
        );
        sw.note_task_error(&id);
        assert_eq!(sw.sweep(), vec![SweepAction::Blocked(id)]);
    }

    #[test]
    fn full_track_carries_deadline_through_adapter() {
        // Deadline path needs time travel the adapter cannot inject, so
        // assert the track call itself is accepted and silent when fresh.
        let mut sw = FleetSweeper::new();
        let id = TaskId::new();
        sw.track_task_full(id, 2, Some(3600));
        assert!(sw.sweep().is_empty());
        sw.untrack_task(&id);
        assert!(sw.sweep().is_empty());
    }
}
