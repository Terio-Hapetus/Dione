//! Compiles the visible "context window" view-model from the mirrored state.
//!
//! M3d: reads the agent-agnostic `transcripts`/`costs` — no opencode wire
//! types. Token estimates use the chars/4 heuristic, anchored by the real
//! accumulated cost totals.

use crate::state::Store;
use crate::transcript::Role;

#[derive(Debug, Clone)]
pub struct ContextSection {
    pub label: String,
    pub kind: SectionKind,
    pub detail: String,
    pub est_tokens: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    System,
    User,
    Assistant,
    Reasoning,
    ToolCall,
    Other,
}

#[derive(Debug, Clone, Default)]
pub struct ContextView {
    pub sections: Vec<ContextSection>,
    pub est_total_tokens: usize,
    pub actual_input_tokens: Option<f64>,
    pub actual_cache_read: Option<f64>,
    pub actual_output_tokens: Option<f64>,
    pub actual_total: Option<f64>,
}

pub fn est_tokens(text: &str) -> usize {
    text.chars().count() / 4 + usize::from(!text.is_empty())
}

pub fn compile(store: &Store) -> ContextView {
    let mut view = ContextView::default();
    view.sections.push(ContextSection {
        label: "system prompt".into(),
        kind: SectionKind::System,
        detail: "injected by the agent (not exposed to ADE)".into(),
        est_tokens: 2_000,
    });

    let Some(sid) = store.active_session.as_deref() else {
        view.est_total_tokens = view.sections.iter().map(|s| s.est_tokens).sum();
        return view;
    };
    for m in store.transcript_for_session(sid) {
        if m.text.trim().is_empty() {
            continue;
        }
        let (label, kind) = match m.role {
            Role::User => ("user".to_string(), SectionKind::User),
            Role::Agent => ("assistant".into(), SectionKind::Assistant),
            Role::Tool => (
                m.tool
                    .as_ref()
                    .map(|t| format!("tool:{}", t.name))
                    .unwrap_or_else(|| "tool".into()),
                SectionKind::ToolCall,
            ),
        };
        view.sections.push(ContextSection {
            label,
            kind,
            detail: truncate(&m.text, 400),
            est_tokens: est_tokens(&m.text),
        });
    }

    if let Some(cost) = store.session_task.get(sid).and_then(|t| store.costs.get(t)) {
        view.actual_input_tokens = Some(cost.input);
        view.actual_cache_read = Some(cost.cache);
        view.actual_output_tokens = Some(cost.output);
        view.actual_total = Some(cost.input + cost.cache + cost.output);
    }

    view.est_total_tokens = view.sections.iter().map(|s| s.est_tokens).sum();
    view
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}
