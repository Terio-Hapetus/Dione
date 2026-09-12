use ade_core::TaskId;

/// Default retry budget: how many errors before a task is `Blocked`.
/// Override per task with [`Task::with_failure_limit`].
pub const DEFAULT_FAILURE_LIMIT: u8 = 2;

/// Liveness of one task (M7a retry budget). Separate from the legacy
/// `WorktreeStatus`, which stays session-derived until M7c unifies identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskStatus {
    /// Running or idle, eligible for sweep reclaim.
    #[default]
    Active,
    /// Hit an error but still under budget; next success clears it.
    Retrying,
    /// Hit `failure_limit` errors: auto-retry stops, needs a human look.
    /// Cleared only by an explicit manual retry.
    Blocked,
}

/// One unit of agent work: one task = one isolated worktree (M2 layout:
/// `<repo>/.ade-worktrees/<slug>`, branch `ade/<slug>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: TaskId,
    pub slug: String,
    /// Which agent backend runs it, e.g. `"opencode"`, `"claude"`.
    /// Resolved via `agents.toml` (M4); free-form until then.
    pub agent_ref: String,
    /// Per-task model override; `None` inherits the session default.
    pub model_override: Option<(String, String)>,
    /// Soft cap in seconds; the dispatcher (M7) reclaims past it.
    pub max_runtime_secs: Option<u64>,
    /// Retry budget (M7a); `Dispatcher` blocks past this many errors.
    pub failure_limit: u8,
    /// Consecutive errors so far; reset by success or manual retry.
    pub failure_count: u32,
    /// Liveness; only a manual retry clears `Blocked`.
    pub status: TaskStatus,
}

impl Task {
    pub fn new(slug: &str, agent_ref: &str) -> Self {
        Self {
            id: TaskId::new(),
            slug: slug.to_string(),
            agent_ref: agent_ref.to_string(),
            model_override: None,
            max_runtime_secs: None,
            failure_limit: DEFAULT_FAILURE_LIMIT,
            failure_count: 0,
            status: TaskStatus::Active,
        }
    }

    pub fn with_model(mut self, provider_id: &str, model_id: &str) -> Self {
        self.model_override = Some((provider_id.to_string(), model_id.to_string()));
        self
    }

    pub fn with_failure_limit(mut self, limit: u8) -> Self {
        self.failure_limit = limit.max(1);
        self
    }

    /// Record one backend error. Returns the new status: `Blocked` once
    /// `failure_count` reaches `failure_limit`, else `Retrying`.
    /// Already-`Blocked` tasks stay blocked (no counter growth).
    pub fn note_failure(&mut self) -> TaskStatus {
        if self.status == TaskStatus::Blocked {
            return TaskStatus::Blocked;
        }
        self.failure_count = self.failure_count.saturating_add(1);
        self.status = if self.failure_count >= u32::from(self.failure_limit.max(1)) {
            TaskStatus::Blocked
        } else {
            TaskStatus::Retrying
        };
        self.status
    }

    /// Record one success: back to `Active`, budget restored.
    pub fn note_success(&mut self) {
        if self.status != TaskStatus::Blocked {
            self.failure_count = 0;
            self.status = TaskStatus::Active;
        }
    }

    /// Manual retry (M7b wires the UI button): clears `Blocked` too.
    pub fn retry_reset(&mut self) {
        self.failure_count = 0;
        self.status = TaskStatus::Active;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_task_has_unique_ids() {
        let a = Task::new("feat-a", "opencode");
        let b = Task::new("feat-a", "opencode");
        assert_ne!(a.id, b.id);
        assert_eq!(a.slug, "feat-a");
        assert!(a.model_override.is_none());
    }

    #[test]
    fn model_override_builder() {
        let t = Task::new("x", "claude").with_model("anthropic", "sonnet");
        assert_eq!(
            t.model_override,
            Some(("anthropic".into(), "sonnet".into()))
        );
    }

    #[test]
    fn default_budget_blocks_on_second_error() {
        let mut t = Task::new("x", "mock");
        assert_eq!(t.failure_limit, DEFAULT_FAILURE_LIMIT);
        assert_eq!(t.status, TaskStatus::Active);
        assert_eq!(t.note_failure(), TaskStatus::Retrying);
        assert_eq!(t.note_failure(), TaskStatus::Blocked);
        // Blocked sticks: no counter growth, success can't clear it.
        assert_eq!(t.note_failure(), TaskStatus::Blocked);
        assert_eq!(t.failure_count, 2);
        t.note_success();
        assert_eq!(t.status, TaskStatus::Blocked);
    }

    #[test]
    fn custom_limit_and_manual_retry() {
        let mut t = Task::new("x", "mock").with_failure_limit(1);
        assert_eq!(t.note_failure(), TaskStatus::Blocked);
        t.retry_reset();
        assert_eq!(t.status, TaskStatus::Active);
        assert_eq!(t.failure_count, 0);
        // Success below budget restores Active.
        let mut u = Task::new("y", "mock").with_failure_limit(3);
        assert_eq!(u.note_failure(), TaskStatus::Retrying);
        u.note_success();
        assert_eq!(u.status, TaskStatus::Active);
        assert_eq!(u.failure_count, 0);
        // Zero limit clamps to 1 (a fresh task must never sweep-blocked).
        assert_eq!(
            Task::new("z", "mock").with_failure_limit(0).failure_limit,
            1
        );
    }
}
