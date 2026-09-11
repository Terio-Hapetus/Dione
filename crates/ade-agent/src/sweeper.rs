//! M4 sweeper adapter: `Dispatcher` behind the `ade-core` seam.
//!
//! The runtime owns the 60s cadence (`sweep_due`) and the consumer
//! (`apply_sweep`); this adapter only translates between the two
//! vocabularies so neither crate names the other's types.

use ade_core::runtime::fleet::{SweepAction, TaskSweeper};
use ade_core::transcript::TaskId;
use ade_workspace::{Dispatcher, DispatcherAction};

/// `ade-workspace::Dispatcher` as an `ade-core::TaskSweeper`.
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

    fn untrack_task(&mut self, task: &TaskId) {
        self.inner.untrack(task);
    }

    fn note_task_error(&mut self, task: &TaskId) {
        self.inner.note_error(task);
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
    use ade_core::runtime::fleet::apply_sweep;
    use ade_core::state::Store;

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
        use ade_core::runtime::fleet::SweepAction;

        let mut store = Store::default();
        apply_sweep(&mut store, &[SweepAction::Reclaim(TaskId::new())]);
        assert!(store.errors[0].contains("reclaim"));
    }
}
