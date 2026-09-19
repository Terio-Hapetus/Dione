//! M12a-d memory (per-repo, orchestrator-side, suggest-only).
//!
//! `RepoMemory` is the in-memory log (cap 50, FIFO). `distill_entry` is the
//! pure hook that turns a closing [`Task`] (Done/Blocked + summary/verdict)
//! into one candidate [`MemoryEntry`] without LLM or hallucination: the text
//! comes only from `task.summary` (first line, trimmed, capped 140 chars) or
//! falls back to the slug. `propose_agents_patch` (M12c) renders entries as
//! a patch suggestion for `AGENTS.md` — never auto-writes — and `recall_context`
//! (M12d) recalls them at dispatch as a context block.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base::TaskId;
use serde::{Deserialize, Serialize};

use crate::task::{Task, TaskStatus};

/// Max entries per repo in-memory (matches the 15-worktree retention spirit;
/// enough history without growing unbounded; disk persistence lands later).
pub const MEMORY_CAP: usize = 50;

/// Max chars for `MemoryEntry.text`. Longer summaries are truncated with `…`,
/// counted by chars (Unicode-safe), mirroring `handoff_summary`.
pub const MEMORY_TEXT_CAP: usize = 140;

/// Outcome class for the orchestrator (not a model score). `Win` = human
/// merged the winner, `Fail` = blocked/abandoned, `Note` = neutral tip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryKind {
    Win,
    Fail,
    Note,
}

impl MemoryKind {
    /// Short label used in patches and recall blocks (single source so
    /// both formats can never drift apart).
    pub fn label(self) -> &'static str {
        match self {
            MemoryKind::Win => "Win",
            MemoryKind::Fail => "Fail",
            MemoryKind::Note => "Note",
        }
    }
}

/// Map a task liveness to the automatic kind (G3): `Blocked→Fail`,
/// anything else→`Note`. `Win` is never automatic — only a human merge
/// verdict assigns it, so callers pass `Win` explicitly.
pub fn kind_for_status(status: TaskStatus) -> MemoryKind {
    match status {
        TaskStatus::Blocked => MemoryKind::Fail,
        TaskStatus::Active | TaskStatus::Retrying => MemoryKind::Note,
    }
}

/// Collapse one logical line: trim, drop `\r`, join inner whitespace runs
/// (including stray newlines in slugs) into single spaces. Keeps entries
/// one-line so patch/recall `lines().count()` stays exact and sentinel
/// markers cannot be smuggled inside an entry.
fn one_line(s: &str) -> String {
    s.replace('\r', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Truncate to exactly `max` chars (Unicode scalar, like `handoff_summary`):
/// shorter input passes through, over-long input keeps `max-1` chars plus
/// a trailing `…` for exactly `max`.
fn truncate_exact(s: &str, max: usize) -> String {
    let max = max.max(1);
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

/// One distilled fact per closed task. `text` is the only human-visible
/// payload; the other fields are for filtering and audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub task: TaskId,
    pub slug: String,
    pub agent_ref: String,
    pub kind: MemoryKind,
    /// Short fact (≤140 chars, first line of `Task.summary` or slug fallback).
    pub text: String,
    /// Unix seconds (caller-provided for deterministic tests).
    pub ts: u64,
}

/// Append-only per-repo log, capped at [`MEMORY_CAP`] (FIFO eviction).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RepoMemory {
    entries: Vec<MemoryEntry>,
}

impl RepoMemory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[MemoryEntry] {
        &self.entries
    }

    /// Record one entry; oldest is dropped once the cap is hit.
    pub fn record(&mut self, entry: MemoryEntry) {
        self.entries.push(entry);
        if self.entries.len() > MEMORY_CAP {
            self.entries.remove(0);
        }
    }

    /// Most-recent `take` entries (capped to `len`); shared by the patch
    /// and recall renderers so both can never disagree on recency.
    pub fn recent(&self, take: usize) -> &[MemoryEntry] {
        if take == 0 || self.is_empty() {
            return &[];
        }
        let n = take.min(self.len());
        &self.entries[self.len() - n..]
    }

    /// Distill one closed task and record it in one step; `None` when the
    /// task has nothing worth remembering.
    pub fn record_distilled(
        &mut self,
        task: &Task,
        kind: MemoryKind,
        ts: u64,
    ) -> Option<MemoryEntry> {
        let e = distill_entry(task, kind, ts)?;
        self.record(e.clone());
        Some(e)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Per-repo owner (G3 wiring): one `RepoMemory` per canonical repo path.
/// Lives in the desktop app (`DioneApp`); the runtime crates stay
/// repo-agnostic. In-memory (like `Store.blocked`); disk persistence lands
/// in a later slice.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore {
    map: BTreeMap<PathBuf, RepoMemory>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mutable memory for a repo, creating it on first use. Callers pass
    /// the canonical repo root (the same key as `container_name_for`).
    pub fn memory_for(&mut self, repo: &Path) -> &mut RepoMemory {
        self.map.entry(repo.to_path_buf()).or_default()
    }

    pub fn get(&self, repo: &Path) -> Option<&RepoMemory> {
        self.map.get(repo)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Pure distill hook (M12b): closed task → one candidate entry, or `None`
/// when there is nothing worth remembering. Never invents text: the source
/// is `task.summary` (trimmed, first line) or, when blank, the slug. Over-
/// long text is cut to [`MEMORY_TEXT_CAP`] chars with a trailing `…`.
pub fn distill_entry(task: &Task, kind: MemoryKind, ts: u64) -> Option<MemoryEntry> {
    // Sanitized slug doubles as fallback text and as the stored slug, so
    // both can never disagree (old code trimmed only the text copy).
    let slug = one_line(&task.slug);
    let raw = task.summary.as_deref().unwrap_or("").trim();
    let first = if raw.is_empty() {
        slug.clone()
    } else {
        one_line(raw.lines().next().unwrap_or(""))
    };
    if first.is_empty() || slug.is_empty() {
        return None;
    }
    // Exactly MEMORY_TEXT_CAP chars max (old code emitted cap+1).
    let text = truncate_exact(&first, MEMORY_TEXT_CAP);
    Some(MemoryEntry {
        task: task.id,
        slug,
        agent_ref: task.agent_ref.clone(),
        kind,
        text,
        ts,
    })
}

/// M12c — suggest-only patch for `AGENTS.md` (pure, never writes).
/// Returns `None` when there is nothing to propose (empty memory or
/// `take == 0`). Otherwise renders the most-recent `take` entries (capped
/// to `len`) as a fenced snippet the UI shows in the File/Diff viewer;
/// the human must press **Apply** — the file is never touched here.
/// Sentinel markers framing the managed block inside `AGENTS.md`. The UI
/// Apply path (`merge_into_agents_md`) replaces between them idempotently.
pub const MEMORY_START: &str = "<!-- dione:memory:start -->";
pub const MEMORY_END: &str = "<!-- dione:memory:end -->";

fn format_entry(e: &MemoryEntry) -> String {
    format!("- [{}] {}: {}", e.kind.label(), e.slug, e.text)
}

pub fn propose_agents_patch(memory: &RepoMemory, take: usize) -> Option<String> {
    let slice = memory.recent(take);
    if slice.is_empty() {
        return None;
    }
    let mut out = String::from("## Dione Memory (suggest-only — human approves before write)\n\n");
    out.push_str(MEMORY_START);
    out.push('\n');
    for e in slice {
        out.push_str(&format_entry(e));
        out.push('\n');
    }
    out.push_str(MEMORY_END);
    out.push('\n');
    Some(out)
}

/// Idempotent Apply helper (pure): if `existing` already carries the
/// managed block, that block is replaced; otherwise the patch is appended
/// (ensuring exactly one blank line of separation). Never touches disk —
/// the caller writes the returned string after human approval.
pub fn merge_into_agents_md(existing: &str, patch: &str) -> String {
    let inner: Vec<String> = patch
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    let block = inner.join("\n");
    if let (Some(s), Some(e)) = (existing.find(MEMORY_START), existing.find(MEMORY_END))
        && s < e
    {
        let end = e + MEMORY_END.len();
        let mut out = String::from(existing[..s].trim_end());
        out.push_str("\n\n");
        out.push_str(&block);
        out.push('\n');
        let rest = existing[end..].trim_start_matches('\n');
        if !rest.trim().is_empty() {
            out.push('\n');
            out.push_str(rest.trim_end());
            out.push('\n');
        }
        return out;
    }
    let mut out = String::from(existing.trim_end());
    out.push_str("\n\n");
    out.push_str(&block);
    out.push('\n');
    out
}

/// M12d — recall helper (pure). Formats the most-recent `take` entries
/// as a short context block to inject into a child/retry prompt. `None`
/// when there is nothing to recall; caller decides how to join it with
/// `Task.summary` (e.g. `format!("{recall}\n\n{parent_summary}")`).
pub fn recall_context(memory: &RepoMemory, take: usize) -> Option<String> {
    let slice = memory.recent(take);
    if slice.is_empty() {
        return None;
    }
    Some(
        slice
            .iter()
            .map(format_entry)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Recall with a total budget (chars): takes the most-recent entries that
/// fit, so a 50-entry memory can never blow up a child prompt. Returns
/// `None` when even the newest entry exceeds the budget. `max_chars < 1`
/// is treated as 1.
pub fn recall_context_capped(memory: &RepoMemory, take: usize, max_chars: usize) -> Option<String> {
    let slice = memory.recent(take);
    if slice.is_empty() {
        return None;
    }
    let max = max_chars.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut len = 0usize;
    // Walk newest-first so the freshest entries win the budget, then
    // restore chronological order for the prompt.
    for e in slice.iter().rev() {
        let line = format_entry(e);
        let add = line.chars().count() + if lines.is_empty() { 0 } else { 1 };
        if len + add > max {
            break;
        }
        len += add;
        lines.push(line);
    }
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{Task, TaskStatus};

    #[test]
    fn empty_starts_clean() {
        let m = RepoMemory::new();
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
        assert!(m.entries().is_empty());
    }

    #[test]
    fn cap_evicts_oldest_fifo() {
        let mut m = RepoMemory::new();
        for n in 0..MEMORY_CAP + 5 {
            let task = Task::new(&format!("s{n}"), "mock").with_summary(format!("note {n}"));
            let e = distill_entry(&task, MemoryKind::Note, n as u64).unwrap();
            m.record(e);
        }
        assert_eq!(m.len(), MEMORY_CAP);
        // first 5 evicted, so entry 5 is now oldest
        assert_eq!(m.entries()[0].slug, "s5");
        assert_eq!(
            m.entries()[MEMORY_CAP - 1].slug,
            format!("s{}", MEMORY_CAP + 4)
        );
    }

    #[test]
    fn distill_prefers_summary_over_slug() {
        let task = Task::new("feat-x", "claude").with_summary("  use cargo test -p base  \nsecond");
        let e = distill_entry(&task, MemoryKind::Win, 42).unwrap();
        assert_eq!(e.text, "use cargo test -p base");
        assert_eq!(e.slug, "feat-x");
        assert_eq!(e.agent_ref, "claude");
        assert_eq!(e.kind, MemoryKind::Win);
        assert_eq!(e.ts, 42);
    }

    #[test]
    fn distill_falls_back_to_slug_when_summary_blank() {
        let task = Task::new("  feat-y  ", "mock");
        let e = distill_entry(&task, MemoryKind::Fail, 7).unwrap();
        assert_eq!(e.text, "feat-y");
        // Stored slug is sanitized too (was kept whitespace-padded).
        assert_eq!(e.slug, "feat-y");
        // whitespace-only summary also falls back
        let t2 = Task::new("feat-z", "mock").with_summary("   \n  ");
        let e2 = distill_entry(&t2, MemoryKind::Note, 7).unwrap();
        assert_eq!(e2.text, "feat-z");
    }

    #[test]
    fn distill_returns_none_for_empty_slug_and_summary() {
        let task = Task::new("   ", "mock");
        assert!(distill_entry(&task, MemoryKind::Note, 0).is_none());
    }

    #[test]
    fn distill_caps_long_text_with_ellipsis() {
        let long = "a".repeat(MEMORY_TEXT_CAP + 10);
        let task = Task::new("s", "mock").with_summary(long);
        let e = distill_entry(&task, MemoryKind::Note, 1).unwrap();
        // Exactly the cap (was cap+1 before the G3 fix).
        assert_eq!(e.text.chars().count(), MEMORY_TEXT_CAP);
        assert!(e.text.ends_with('…'));
        assert!(e.text.starts_with('a'));
    }

    #[test]
    fn distill_keeps_exact_cap_without_ellipsis() {
        let exact = "b".repeat(MEMORY_TEXT_CAP);
        let task = Task::new("s", "mock").with_summary(exact.clone());
        let e = distill_entry(&task, MemoryKind::Note, 1).unwrap();
        assert_eq!(e.text, exact);
        assert!(!e.text.ends_with('…'));
    }

    #[test]
    fn kind_and_status_are_independent() {
        // Hook does not look at TaskStatus; caller maps Blocked→Fail etc.
        let mut task = Task::new("s", "mock").with_summary("tip");
        task.status = TaskStatus::Blocked;
        let e = distill_entry(&task, MemoryKind::Note, 0).unwrap();
        assert_eq!(e.kind, MemoryKind::Note);
        let e2 = distill_entry(&task, MemoryKind::Fail, 0).unwrap();
        assert_eq!(e2.kind, MemoryKind::Fail);
    }

    #[test]
    fn clear_empties() {
        let mut m = RepoMemory::new();
        let t = Task::new("s", "mock").with_summary("x");
        m.record(distill_entry(&t, MemoryKind::Note, 0).unwrap());
        assert_eq!(m.len(), 1);
        m.clear();
        assert!(m.is_empty());
    }

    #[test]
    fn propose_patch_is_suggest_only_and_most_recent() {
        let mut m = RepoMemory::new();
        assert_eq!(propose_agents_patch(&m, 3), None);
        assert_eq!(propose_agents_patch(&m, 0), None);
        for (slug, kind, text) in [
            ("a", MemoryKind::Win, "first win"),
            ("b", MemoryKind::Fail, "then fail"),
            ("c", MemoryKind::Note, "tip"),
        ] {
            let t = Task::new(slug, "mock").with_summary(text);
            m.record(distill_entry(&t, kind, 1).unwrap());
        }
        let p = propose_agents_patch(&m, 2).unwrap();
        // Header + sentinels, never writes itself.
        assert!(p.contains("## Dione Memory (suggest-only"));
        assert!(p.contains("<!-- dione:memory:start -->"));
        assert!(p.contains("<!-- dione:memory:end -->"));
        // Most-recent 2: b then c, not a.
        assert!(!p.contains("first win"));
        assert!(p.contains("- [Fail] b: then fail"));
        assert!(p.contains("- [Note] c: tip"));
        // Take beyond len caps.
        let all = propose_agents_patch(&m, 10).unwrap();
        assert!(all.contains("first win"));
        assert_eq!(all.matches("- [").count(), 3);
    }

    #[test]
    fn propose_patch_and_recall_share_format() {
        let mut m = RepoMemory::new();
        let t = Task::new("feat-x", "claude").with_summary("do X before Y");
        m.record(distill_entry(&t, MemoryKind::Win, 9).unwrap());
        let patch = propose_agents_patch(&m, 1).unwrap();
        assert!(patch.contains("- [Win] feat-x: do X before Y"));
        let ctx = recall_context(&m, 1).unwrap();
        assert_eq!(ctx, "- [Win] feat-x: do X before Y");
    }

    #[test]
    fn recall_none_when_empty_or_zero() {
        let m = RepoMemory::new();
        assert_eq!(recall_context(&m, 3), None);
        let mut m2 = RepoMemory::new();
        let t = Task::new("s", "mock").with_summary("hi");
        m2.record(distill_entry(&t, MemoryKind::Note, 0).unwrap());
        assert_eq!(recall_context(&m2, 0), None);
    }

    #[test]
    fn recall_takes_most_recent_and_capped() {
        let mut m = RepoMemory::new();
        for n in 0..5 {
            let t = Task::new(&format!("s{n}"), "mock").with_summary(format!("n{n}"));
            m.record(distill_entry(&t, MemoryKind::Note, n).unwrap());
        }
        let ctx = recall_context(&m, 2).unwrap();
        assert!(ctx.contains("s3"));
        assert!(ctx.contains("s4"));
        assert!(!ctx.contains("s2"));
        assert_eq!(ctx.lines().count(), 2);
        // beyond len caps to all.
        assert_eq!(recall_context(&m, 10).unwrap().lines().count(), 5);
    }

    #[test]
    fn kind_mapper_and_labels() {
        assert_eq!(kind_for_status(TaskStatus::Blocked), MemoryKind::Fail);
        assert_eq!(kind_for_status(TaskStatus::Active), MemoryKind::Note);
        assert_eq!(kind_for_status(TaskStatus::Retrying), MemoryKind::Note);
        assert_eq!(MemoryKind::Win.label(), "Win");
        assert_eq!(MemoryKind::Fail.label(), "Fail");
        assert_eq!(MemoryKind::Note.label(), "Note");
    }

    #[test]
    fn distill_sanitizes_slug_newlines() {
        let task = Task::new("a\n- [Win] evil: x", "mock").with_summary("tip");
        let e = distill_entry(&task, MemoryKind::Note, 0).unwrap();
        assert!(!e.slug.contains('\n'));
        assert_eq!(e.slug, "a - [Win] evil: x");
        // One logical line: recall stays single-line per entry.
        let mut m = RepoMemory::new();
        m.record(e);
        assert_eq!(recall_context(&m, 1).unwrap().lines().count(), 1);
    }

    #[test]
    fn merge_replaces_block_idempotently() {
        let mut m = RepoMemory::new();
        let t = Task::new("s", "mock").with_summary("tip");
        m.record(distill_entry(&t, MemoryKind::Note, 0).unwrap());
        let patch = propose_agents_patch(&m, 5).unwrap();
        let base = "# Title\n\nSome intro.\n";
        let once = merge_into_agents_md(base, &patch);
        assert!(once.contains(MEMORY_START));
        assert!(once.contains("- [Note] s: tip"));
        assert!(once.starts_with("# Title"));
        // Applying again (e.g. after a new entry) replaces, never doubles.
        let t2 = Task::new("s2", "mock").with_summary("tip2");
        m.record(distill_entry(&t2, MemoryKind::Win, 1).unwrap());
        let patch2 = propose_agents_patch(&m, 5).unwrap();
        let twice = merge_into_agents_md(&once, &patch2);
        assert_eq!(twice.matches(MEMORY_START).count(), 1);
        assert_eq!(twice.matches(MEMORY_END).count(), 1);
        assert!(twice.contains("tip2"));
    }

    #[test]
    fn merge_keeps_trailing_content() {
        let patch = "## Dione Memory\n\n<!-- dione:memory:start -->\n- [Note] s: x\n<!-- dione:memory:end -->\n";
        let base =
            "# T\n\n<!-- dione:memory:start -->\nold\n<!-- dione:memory:end -->\n\n## Later\n";
        let out = merge_into_agents_md(base, patch);
        assert!(out.contains("- [Note] s: x"));
        assert!(!out.contains("\nold\n"));
        assert!(out.contains("## Later"));
    }

    #[test]
    fn recall_capped_fits_budget_newest_first() {
        let mut m = RepoMemory::new();
        for n in 0..5 {
            let t = Task::new(&format!("s{n}"), "mock").with_summary(format!("n{n}"));
            m.record(distill_entry(&t, MemoryKind::Note, n).unwrap());
        }
        // Full recall is 5 lines; a tiny budget keeps only the newest.
        let full = recall_context(&m, 5).unwrap();
        assert_eq!(full.lines().count(), 5);
        let small = recall_context_capped(&m, 5, 20).unwrap();
        assert!(small.contains("s4"));
        assert!(!small.contains("s3"));
        // Zero budget clamps to 1 char → newest entry (14 chars) exceeds → None.
        assert_eq!(recall_context_capped(&m, 5, 0), None);
        // Generous budget returns everything in order.
        assert_eq!(
            recall_context_capped(&m, 5, 10_000)
                .unwrap()
                .lines()
                .count(),
            5
        );
    }

    #[test]
    fn store_owns_one_memory_per_repo() {
        use std::path::Path;
        let mut store = MemoryStore::new();
        assert!(store.is_empty());
        let t = Task::new("s", "mock").with_summary("tip");
        store
            .memory_for(Path::new("/repo-a"))
            .record(distill_entry(&t, MemoryKind::Note, 0).unwrap());
        assert_eq!(store.len(), 1);
        assert_eq!(store.get(Path::new("/repo-a")).unwrap().len(), 1);
        assert!(store.get(Path::new("/repo-b")).is_none());
        // Same repo path returns the same memory.
        store.memory_for(Path::new("/repo-a")).record_distilled(
            &Task::new("s2", "mock").with_summary("t2"),
            MemoryKind::Win,
            1,
        );
        assert_eq!(store.get(Path::new("/repo-a")).unwrap().len(), 2);
    }

    #[test]
    fn recall_is_pure_and_never_writes() {
        let mut m = RepoMemory::new();
        let t = Task::new("s", "mock").with_summary("tip");
        m.record(distill_entry(&t, MemoryKind::Note, 0).unwrap());
        let before = m.len();
        let c = recall_context(&m, 1).unwrap();
        assert_eq!(m.len(), before);
        assert!(c.contains("tip"));
        let p = propose_agents_patch(&m, 1).unwrap();
        assert_eq!(m.len(), before);
        assert!(p.contains("tip"));
    }
}
