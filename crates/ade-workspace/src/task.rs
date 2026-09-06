use ade_core::TaskId;

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
}

impl Task {
    pub fn new(slug: &str, agent_ref: &str) -> Self {
        Self {
            id: TaskId::new(),
            slug: slug.to_string(),
            agent_ref: agent_ref.to_string(),
            model_override: None,
            max_runtime_secs: None,
        }
    }

    pub fn with_model(mut self, provider_id: &str, model_id: &str) -> Self {
        self.model_override = Some((provider_id.to_string(), model_id.to_string()));
        self
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
}
