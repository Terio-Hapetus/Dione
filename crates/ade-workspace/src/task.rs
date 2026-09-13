use ade_core::TaskId;
use ade_core::transcript::{Role, UnifiedMessage};

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
    /// Handoff (M7c): the task this one continues, if any.
    pub parent: Option<TaskId>,
    /// Handoff (M7c): 1–2 lines of context for whoever continues the work.
    /// Set by hand or auto-snapshotted on `Blocked`; inherited by children.
    pub summary: Option<String>,
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
            parent: None,
            summary: None,
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

    pub fn with_parent(mut self, parent: TaskId) -> Self {
        self.parent = Some(parent);
        self
    }

    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Spawn a continuation (M7c handoff): fresh id and budget, same
    /// agent face, parent linked, summary inherited as starting context.
    pub fn child(&self, slug: &str, agent_ref: &str) -> Self {
        Self {
            id: TaskId::new(),
            slug: slug.to_string(),
            agent_ref: agent_ref.to_string(),
            model_override: self.model_override.clone(),
            max_runtime_secs: self.max_runtime_secs,
            failure_limit: self.failure_limit,
            failure_count: 0,
            status: TaskStatus::Active,
            parent: Some(self.id),
            summary: self.summary.clone(),
        }
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

/// Handoff context (M7c): the last `take` messages as `role: text` lines
/// (first line per message), capped at `max_chars`. Empty input → None.
pub fn handoff_summary(msgs: &[UnifiedMessage], take: usize, max_chars: usize) -> Option<String> {
    let start = msgs.len().saturating_sub(take.max(1));
    let mut out = String::new();
    for m in &msgs[start..] {
        let role = match m.role {
            Role::User => "user",
            Role::Agent => "agent",
            Role::Tool => "tool",
        };
        let first = m.text.lines().next().unwrap_or("").trim();
        if first.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(role);
        out.push_str(": ");
        out.push_str(first);
    }
    if out.is_empty() {
        return None;
    }
    let max = max_chars.max(1);
    if out.chars().count() > max {
        out = format!("{}…", out.chars().take(max).collect::<String>());
    }
    Some(out)
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

    #[test]
    fn child_links_parent_and_inherits_context() {
        let mut parent = Task::new("feat-a", "mock")
            .with_failure_limit(3)
            .with_summary("tried X, stuck on Y");
        parent.note_failure();
        let kid = parent.child("feat-a-retry", "mock");
        assert_ne!(kid.id, parent.id);
        assert_eq!(kid.parent, Some(parent.id));
        assert_eq!(kid.summary.as_deref(), Some("tried X, stuck on Y"));
        // Fresh budget, same limit — but the failure streak stays behind.
        assert_eq!(kid.failure_limit, 3);
        assert_eq!(kid.failure_count, 0);
        assert_eq!(kid.status, TaskStatus::Active);
        // Grandchildren point at their direct parent (chain, not root).
        let grand = kid.child("feat-a-retry2", "mock");
        assert_eq!(grand.parent, Some(kid.id));
        assert_eq!(grand.summary, kid.summary);
    }

    #[test]
    fn fresh_task_has_no_parent_or_summary() {
        let t = Task::new("x", "mock");
        assert_eq!(t.parent, None);
        assert_eq!(t.summary, None);
    }

    fn um(task: TaskId, role: Role, text: &str) -> UnifiedMessage {
        UnifiedMessage {
            id: format!("{}-{}", task, text.len()),
            task,
            role,
            text: text.into(),
            tool: None,
            ts: 0,
        }
    }

    #[test]
    fn handoff_summary_takes_tail_and_caps() {
        let id = TaskId::new();
        let msgs = vec![
            um(id, Role::User, "old prompt"),
            um(id, Role::Agent, "did X\nmore detail"),
            um(id, Role::Tool, "   "),
            um(id, Role::Agent, "stuck on Y"),
        ];
        assert_eq!(
            handoff_summary(&msgs, 3, 500).as_deref(),
            Some("agent: did X\nagent: stuck on Y")
        );
        // Empty / whitespace-only input → None.
        assert_eq!(handoff_summary(&[], 3, 500), None);
        assert_eq!(handoff_summary(&msgs[2..3], 3, 500), None);
        // Cap truncates by chars with an ellipsis marker.
        let long = handoff_summary(&msgs[3..], 3, 10);
        assert!(long.is_some_and(|s| s.ends_with('…') && s.chars().count() == 11));
    }
}
