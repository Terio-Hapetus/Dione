//! M4d dispatcher: kanban-lite sweep every 60s (Lab: reclaim + block).
//!
//! The dispatcher never touches backends or the `Store`; it only reports
//! what the runtime should do. M7a wires per-task retry budgets here:
//! `track` copies the task's `failure_limit`, `sweep_at` blocks past it.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ade_core::TaskId;

use super::task::{DEFAULT_FAILURE_LIMIT, Task};

/// Default sweep cadence in seconds (ROADMAP M4).
pub const DISPATCH_INTERVAL_SECS: u64 = 60;

/// What the runtime should do about a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatcherAction {
    /// Overdue or stale: take the task back for reassignment/retry.
    Reclaim(TaskId),
    /// Retry budget spent: stop auto-retry, needs a human look.
    Blocked(TaskId),
}

#[derive(Debug, Clone, Copy)]
struct TaskMeta {
    deadline: Option<Instant>,
    errors: u32,
    limit: u8,
    last_beat: Instant,
}

/// Tracks live tasks and reports reclaim/block actions per sweep.
#[derive(Debug)]
pub struct Dispatcher {
    interval: Duration,
    tasks: BTreeMap<TaskId, TaskMeta>,
}

impl Default for Dispatcher {
    /// Same as `new()`: a zero interval would reclaim every fresh task.
    fn default() -> Self {
        Self::new()
    }
}

impl Dispatcher {
    pub fn new() -> Self {
        Self {
            interval: Duration::from_secs(DISPATCH_INTERVAL_SECS),
            tasks: BTreeMap::new(),
        }
    }

    pub fn track(&mut self, task: &Task) {
        self.track_with_limit(task.id, task.max_runtime_secs, task.failure_limit);
    }

    /// Track a bare id with no deadline (stale-only reclaim). Used by the
    /// `TaskSweeper` adapter, which sees `TaskId`s, not full tasks.
    pub fn track_id(&mut self, id: TaskId) {
        self.track_with_limit(id, None, DEFAULT_FAILURE_LIMIT);
    }

    /// Track a bare id with an explicit retry budget (M7a seam: the
    /// runtime forwards each task's `failure_limit` without naming `Task`).
    pub fn track_id_with_limit(&mut self, id: TaskId, limit: u8) {
        self.track_with_limit(id, None, limit);
    }

    /// Track with budget + deadline (fix-2 seam). Re-tracking an already
    /// tracked task keeps its error count — only fresh tracks reset it
    /// (retry goes through untrack + re-track explicitly).
    pub fn track_full(&mut self, id: TaskId, limit: u8, max_runtime_secs: Option<u64>) {
        self.track_with_limit(id, max_runtime_secs, limit);
    }

    fn track_with_limit(&mut self, id: TaskId, max_runtime_secs: Option<u64>, limit: u8) {
        let now = Instant::now();
        self.tasks.entry(id).or_insert(TaskMeta {
            deadline: max_runtime_secs.map(|s| now + Duration::from_secs(s)),
            errors: 0,
            limit: limit.max(1),
            last_beat: now,
        });
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

    /// Record a success (fix-2): the error streak resets so an
    /// error→success→error cycle under budget never blocks.
    pub fn note_success(&mut self, task: &TaskId) {
        if let Some(m) = self.tasks.get_mut(task) {
            m.errors = 0;
            m.last_beat = Instant::now();
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
            if m.errors >= u32::from(m.limit) {
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
    fn bare_id_tracks_without_deadline() {
        let mut d = Dispatcher::new();
        let id = TaskId::new();
        d.track_id(id);
        assert!(d.sweep().is_empty());
        d.note_error(&id);
        d.note_error(&id);
        assert_eq!(d.sweep(), vec![DispatcherAction::Blocked(id)]);
        d.untrack(&id);
        assert!(d.sweep().is_empty());
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

    #[test]
    fn per_task_limit_moves_block_threshold() {
        // limit 1: first error blocks immediately.
        let mut d = Dispatcher::new();
        let t = Task::new("a", "mock").with_failure_limit(1);
        d.track(&t);
        d.note_error(&t.id);
        assert_eq!(d.sweep(), vec![DispatcherAction::Blocked(t.id)]);
    }

    #[test]
    fn per_task_limit_tolerates_more_errors() {
        // limit 3: two errors are still silent.
        let mut d = Dispatcher::new();
        let t = Task::new("a", "mock").with_failure_limit(3);
        d.track(&t);
        d.note_error(&t.id);
        d.note_error(&t.id);
        assert!(d.sweep().is_empty());
        d.note_error(&t.id);
        assert_eq!(d.sweep(), vec![DispatcherAction::Blocked(t.id)]);
    }

    #[test]
    fn bare_id_limit_override() {
        let mut d = Dispatcher::new();
        let id = TaskId::new();
        d.track_id_with_limit(id, 1);
        d.note_error(&id);
        assert_eq!(d.sweep(), vec![DispatcherAction::Blocked(id)]);
    }

    #[test]
    fn success_resets_error_streak() {
        let mut d = Dispatcher::new();
        let t = Task::new("a", "mock"); // limit 2
        d.track(&t);
        d.note_error(&t.id);
        d.note_success(&t.id);
        d.note_error(&t.id);
        assert!(
            d.sweep().is_empty(),
            "error→success→error stays under budget"
        );
        d.note_error(&t.id);
        assert_eq!(d.sweep(), vec![DispatcherAction::Blocked(t.id)]);
    }

    #[test]
    fn retrack_keeps_errors_until_untrack() {
        let mut d = Dispatcher::new();
        let t = Task::new("a", "mock");
        d.track(&t);
        d.note_error(&t.id);
        d.track(&t); // second bind must not wipe the count
        d.note_error(&t.id);
        assert_eq!(d.sweep(), vec![DispatcherAction::Blocked(t.id)]);
        // Retry path (untrack + re-track) starts fresh.
        d.untrack(&t.id);
        d.track(&t);
        assert!(d.sweep().is_empty());
    }

    #[test]
    fn full_track_carries_deadline() {
        let mut d = Dispatcher::new();
        let id = TaskId::new();
        d.track_full(id, 2, Some(0));
        let now = Instant::now() + Duration::from_secs(1);
        assert_eq!(d.sweep_at(now), vec![DispatcherAction::Reclaim(id)]);
    }
}
