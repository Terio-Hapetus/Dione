//! M4d dispatcher: kanban-lite sweep every 60s (Lab: reclaim + block).
//!
//! The dispatcher never touches backends or the `Store`; it only reports
//! what the runtime should do. Full retry budgets and circuit breakers
//! land in M7 — here `2 errors → Blocked` is just the hook.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ade_core::TaskId;

use super::task::Task;

/// Default sweep cadence in seconds (ROADMAP M4).
pub const DISPATCH_INTERVAL_SECS: u64 = 60;

/// What the runtime should do about a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatcherAction {
    /// Overdue or stale: take the task back for reassignment/retry.
    Reclaim(TaskId),
    /// Errored twice in a row: stop auto-retry, needs a human look.
    Blocked(TaskId),
}

#[derive(Debug, Clone, Copy)]
struct TaskMeta {
    deadline: Option<Instant>,
    errors: u32,
    last_beat: Instant,
}

/// Tracks live tasks and reports reclaim/block actions per sweep.
#[derive(Debug, Default)]
pub struct Dispatcher {
    interval: Duration,
    tasks: BTreeMap<TaskId, TaskMeta>,
}

impl Dispatcher {
    pub fn new() -> Self {
        Self {
            interval: Duration::from_secs(DISPATCH_INTERVAL_SECS),
            tasks: BTreeMap::new(),
        }
    }

    pub fn track(&mut self, task: &Task) {
        let now = Instant::now();
        self.tasks.insert(
            task.id,
            TaskMeta {
                deadline: task.max_runtime_secs.map(|s| now + Duration::from_secs(s)),
                errors: 0,
                last_beat: now,
            },
        );
    }

    pub fn untrack(&mut self, task: &TaskId) {
        self.tasks.remove(task);
    }

    pub fn note_heartbeat(&mut self, task: &TaskId) {
        if let Some(m) = self.tasks.get_mut(task) {
            m.last_beat = Instant::now();
        }
    }

    pub fn note_error(&mut self, task: &TaskId) {
        if let Some(m) = self.tasks.get_mut(task) {
            m.errors = m.errors.saturating_add(1);
        }
    }

    /// Sweep with the real clock.
    pub fn sweep(&self) -> Vec<DispatcherAction> {
        self.sweep_at(Instant::now())
    }

    /// Sweep at an injected time (deterministic tests).
    pub fn sweep_at(&self, now: Instant) -> Vec<DispatcherAction> {
        let mut out = Vec::new();
        for (id, m) in &self.tasks {
            if m.errors >= 2 {
                out.push(DispatcherAction::Blocked(*id));
            } else if m.deadline.is_some_and(|d| now >= d)
                || now.duration_since(m.last_beat) > 2 * self.interval
            {
                out.push(DispatcherAction::Reclaim(*id));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_task_has_no_action() {
        let d = Dispatcher::new();
        let mut d = d;
        let t = Task::new("a", "mock");
        d.track(&t);
        assert!(d.sweep().is_empty());
    }

    #[test]
    fn overdue_task_is_reclaimed() {
        let d = Dispatcher::new();
        let mut d = d;
        let mut t = Task::new("a", "mock");
        t.max_runtime_secs = Some(0);
        d.track(&t);
        let now = Instant::now() + Duration::from_secs(1);
        assert_eq!(d.sweep_at(now), vec![DispatcherAction::Reclaim(t.id)]);
        // Untracked tasks disappear from sweeps.
        d.untrack(&t.id);
        assert!(d.sweep_at(now).is_empty());
    }

    #[test]
    fn two_errors_block_and_win_over_reclaim() {
        let mut d = Dispatcher::new();
        let t = Task::new("a", "mock");
        d.track(&t);
        d.note_error(&t.id);
        assert!(d.sweep().is_empty());
        d.note_error(&t.id);
        let now = Instant::now() + Duration::from_secs(3600);
        assert_eq!(d.sweep_at(now), vec![DispatcherAction::Blocked(t.id)]);
    }

    #[test]
    fn heartbeat_keeps_task_fresh() {
        let mut d = Dispatcher::new();
        let t = Task::new("a", "mock");
        d.track(&t);
        d.note_heartbeat(&t.id);
        assert!(d.sweep().is_empty());
        // Unknown ids are ignored, never crash.
        d.note_heartbeat(&TaskId::new());
        d.note_error(&TaskId::new());
    }
}
