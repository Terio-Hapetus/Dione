use std::collections::{BTreeMap, BTreeSet, VecDeque};

use opencode_codes::protocol_generated::types::{Message, Part, Session, SessionStatus, Todo};

use super::types::{
    ConnState, MessageEntry, PendingPermission, ProviderInfo, SelectedModel, Totals,
};
use crate::transcript::{Cost, TaskId, UnifiedMessage, cost_of_message, entry_to_unified};
use crate::worktree::{WorktreeRecord, WorktreeStatus};

#[derive(Debug, Clone, Default)]
pub struct Store {
    // M0: worktrees + errors.
    pub worktrees: BTreeMap<String, WorktreeRecord>,
    pub active_worktree: Option<String>,
    pub errors: VecDeque<String>,
    // M1: agent mirror.
    pub conn: ConnState,
    pub sessions: BTreeMap<String, Session>,
    pub statuses: BTreeMap<String, SessionStatus>,
    pub messages: BTreeMap<String, Vec<MessageEntry>>,
    pub diffs: BTreeMap<String, serde_json::Value>,
    pub todos: BTreeMap<String, Vec<Todo>>,
    pub pending_permissions: BTreeMap<String, PendingPermission>,
    pub providers: Vec<ProviderInfo>,
    pub selected_model: Option<SelectedModel>,
    pub totals: Totals,
    pub active_session: Option<String>,
    // M2: session id -> "" (repo root) or worktree slug.
    pub session_scope: BTreeMap<String, String>,
    // M3a: agent-agnostic mirror (dual-write with legacy messages).
    pub transcripts: BTreeMap<TaskId, Vec<UnifiedMessage>>,
    pub costs: BTreeMap<TaskId, Cost>,
    pub session_task: BTreeMap<String, TaskId>,
    /// Sessions removed with their worktree: stray in-flight SSE frames for
    /// these ids are ignored instead of resurrecting them.
    pub retired_sessions: BTreeSet<String>,
    /// M7b: tasks stopped by the retry budget (`task -> worktree slug`,
    /// `""` when the task never bound a session). Written by the fleet
    /// sweep, read by the Fleet badge + retry buttons, cleared by manual
    /// retry. In-memory only: a restart rebuilds it from fresh sweeps.
    pub blocked: BTreeMap<TaskId, String>,
}

impl Store {
    // -- M0: worktrees ------------------------------------------------------
    pub fn upsert_worktree(&mut self, record: WorktreeRecord) {
        if self.active_worktree.is_none() {
            self.active_worktree = Some(record.slug.clone());
        }
        self.worktrees.insert(record.slug.clone(), record);
    }

    pub fn remove_worktree(&mut self, slug: &str) -> bool {
        let removed = self.worktrees.remove(slug).is_some();
        if self.active_worktree.as_deref() == Some(slug) {
            self.active_worktree = self.worktrees.keys().next().cloned();
        }
        removed
    }

    pub fn set_active(&mut self, slug: &str) -> bool {
        if self.worktrees.contains_key(slug) {
            self.active_worktree = Some(slug.to_string());
            true
        } else {
            false
        }
    }

    pub fn push_error(&mut self, msg: impl Into<String>) {
        self.errors.push_back(msg.into());
        while self.errors.len() > 20 {
            self.errors.pop_front();
        }
    }

    // -- M7b: blocked mirror ------------------------------------------------
    pub fn mark_blocked(&mut self, task: TaskId, slug: impl Into<String>) {
        self.blocked.insert(task, slug.into());
    }

    pub fn is_blocked_slug(&self, slug: &str) -> bool {
        self.blocked.values().any(|s| s == slug)
    }

    /// Clear every blocked task owned by `slug`; returns their ids so the
    /// caller can re-track them. Unknown slugs clear nothing.
    pub fn clear_blocked_by_slug(&mut self, slug: &str) -> Vec<TaskId> {
        let ids: Vec<TaskId> = self
            .blocked
            .iter()
            .filter(|(_, s)| s.as_str() == slug)
            .map(|(t, _)| *t)
            .collect();
        for id in &ids {
            self.blocked.remove(id);
        }
        ids
    }

    // -- M1: sessions -------------------------------------------------------
    pub fn active_messages(&self) -> Option<&Vec<MessageEntry>> {
        self.active_session
            .as_ref()
            .and_then(|id| self.messages.get(id))
    }

    pub fn is_busy(&self) -> bool {
        self.active_session
            .as_ref()
            .and_then(|id| self.statuses.get(id))
            .is_some_and(|s| matches!(s, SessionStatus::Busy | SessionStatus::Retry { .. }))
    }

    /// M3d: agent-agnostic status helpers so the UI never matches on the
    /// opencode `SessionStatus` wire type directly.
    pub fn is_working(&self, sid: &str) -> bool {
        self.statuses
            .get(sid)
            .is_some_and(|s| matches!(s, SessionStatus::Busy | SessionStatus::Retry { .. }))
    }

    pub fn has_pending(&self, sid: &str) -> bool {
        self.pending_permissions
            .values()
            .any(|p| p.session_id == sid)
    }

    pub fn set_messages(&mut self, sid: &str, entries: Vec<MessageEntry>) {
        self.messages.insert(sid.to_string(), entries);
        self.recompute_totals();
        let snapshot: Vec<(Message, Vec<Part>)> = self
            .messages
            .get(sid)
            .map(|v| {
                v.iter()
                    .map(|e| (e.info.clone(), e.parts.clone()))
                    .collect()
            })
            .unwrap_or_default();
        for (info, parts) in &snapshot {
            self.mirror_entry(sid, info, parts);
        }
    }

    // -- M3a: unified transcript (dual-write) -------------------------------
    /// Stable task id for a session, allocating on first use.
    pub fn task_for_session(&mut self, sid: &str) -> TaskId {
        if let Some(t) = self.session_task.get(sid) {
            return *t;
        }
        let t = TaskId::new();
        self.session_task.insert(sid.to_string(), t);
        t
    }

    /// Insert or replace a unified message by id (idempotent re-mirror).
    pub fn push_unified(&mut self, msg: UnifiedMessage) {
        let list = self.transcripts.entry(msg.task).or_default();
        match list.iter_mut().find(|m| m.id == msg.id) {
            Some(m) => *m = msg,
            None => list.push(msg),
        }
    }

    pub fn transcript_for_session(&self, sid: &str) -> &[UnifiedMessage] {
        match self
            .session_task
            .get(sid)
            .and_then(|t| self.transcripts.get(t))
        {
            Some(v) => v.as_slice(),
            None => &[],
        }
    }

    pub(crate) fn mirror_entry(&mut self, sid: &str, info: &Message, parts: &[Part]) {
        let task = self.task_for_session(sid);
        for u in entry_to_unified(task, info, parts) {
            self.push_unified(u);
        }
        self.recompute_task_cost(task);
    }

    fn recompute_task_cost(&mut self, task: TaskId) {
        let mut c = Cost::default();
        let sids: Vec<String> = self
            .session_task
            .iter()
            .filter(|(_, t)| **t == task)
            .map(|(s, _)| s.clone())
            .collect();
        for sid in &sids {
            if let Some(entries) = self.messages.get(sid) {
                for e in entries {
                    if let Some(mc) = cost_of_message(&e.info) {
                        c.add(mc);
                    }
                }
            }
        }
        self.costs.insert(task, c);
    }

    // -- M2: fleet ----------------------------------------------------------
    /// Scope of a session: `""` for the repo root, else a worktree slug.
    pub fn scope_of(&self, sid: &str) -> &str {
        self.session_scope
            .get(sid)
            .map(String::as_str)
            .unwrap_or("")
    }

    /// Session ids in a scope, newest (by `time.updated`) first.
    pub fn sessions_in_scope(&self, scope: &str) -> Vec<String> {
        let mut ids: Vec<(&String, u64)> = self
            .sessions
            .iter()
            .filter(|(id, _)| self.scope_of(id) == scope)
            .map(|(id, s)| (id, s.time.updated))
            .collect();
        ids.sort_by_key(|(_, updated)| std::cmp::Reverse(*updated));
        ids.into_iter().map(|(id, _)| id.clone()).collect()
    }

    // -- M8b: review queue --------------------------------------------------
    /// Sort key for the review queue: sessions blocked on a human come
    /// first, then longest waiting (oldest `time.updated`) first.
    /// Missing sessions (retired/orphan diffs) sink to the bottom.
    pub fn review_rank(&self, sid: &str) -> (bool, u64) {
        let updated = self
            .sessions
            .get(sid)
            .map(|s| s.time.updated)
            .unwrap_or(u64::MAX);
        (!self.has_pending(sid), updated)
    }

    /// Sort session ids into review order (fair across scopes).
    pub fn sort_review_sids(&self, mut sids: Vec<String>) -> Vec<String> {
        sids.sort_by_key(|sid| self.review_rank(sid));
        sids
    }

    /// Sort scopes by their longest-waiting session with a diff. The main
    /// scope (`""`) competes on equal terms; scopes without diffs sink.
    pub fn sort_review_scopes(&self, mut scopes: Vec<String>) -> Vec<String> {
        scopes.sort_by_key(|scope| {
            self.diffs
                .keys()
                .filter(|sid| self.scope_of(sid) == scope)
                .map(|sid| self.review_rank(sid))
                .min()
                .unwrap_or((true, u64::MAX))
        });
        scopes
    }

    /// Dashboard status for a worktree, derived from its main session.
    pub fn worktree_status(&self, slug: &str) -> WorktreeStatus {
        let Some(record) = self.worktrees.get(slug) else {
            return WorktreeStatus::Creating;
        };
        let Some(sid) = record.session_id.as_deref() else {
            return WorktreeStatus::Creating;
        };
        if self
            .pending_permissions
            .values()
            .any(|p| p.session_id == sid)
        {
            return WorktreeStatus::NeedsYou;
        }
        match self.statuses.get(sid) {
            Some(SessionStatus::Busy) | Some(SessionStatus::Retry { .. }) => {
                WorktreeStatus::Working
            }
            _ => {
                if self.messages.get(sid).is_some_and(|m| !m.is_empty()) {
                    WorktreeStatus::Done
                } else {
                    WorktreeStatus::Creating
                }
            }
        }
    }

    /// Drop the unified mirror + pending gates + totals contribution of a
    /// session. Reads the `session_task` mapping, so callers must invoke it
    /// before that mapping is removed. Supervisor-pushed messages under a
    /// different task id are out of reach (identity unifies in M7).
    pub(crate) fn drop_session_mirror(&mut self, sid: &str) {
        if let Some(task) = self.session_task.get(sid).copied() {
            self.transcripts.remove(&task);
            self.costs.remove(&task);
        }
        self.pending_permissions.retain(|_, p| p.session_id != sid);
        self.recompute_totals();
    }

    /// Drop a session and everything mirrored for it; future SSE frames for
    /// it are ignored. Returns its scope, if known.
    pub fn retire_session(&mut self, sid: &str) -> Option<String> {
        self.sessions.remove(sid);
        self.statuses.remove(sid);
        self.messages.remove(sid);
        self.todos.remove(sid);
        self.diffs.remove(sid);
        self.drop_session_mirror(sid);
        if self.active_session.as_deref() == Some(sid) {
            self.active_session = self.sessions.keys().next().cloned();
        }
        for record in self.worktrees.values_mut() {
            if record.session_id.as_deref() == Some(sid) {
                record.session_id = None;
            }
        }
        self.retired_sessions.insert(sid.to_string());
        self.session_task.remove(sid);
        self.session_scope.remove(sid)
    }

    pub(crate) fn recompute_totals(&mut self) {
        let mut t = Totals::default();
        for entries in self.messages.values() {
            for e in entries {
                if let Message::Assistant(a) = &e.info {
                    t.input += a.tokens.input;
                    t.cache_read += a.tokens.cache.read;
                    t.output += a.tokens.output;
                    t.cost += a.cost;
                }
            }
        }
        self.totals = t;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::WorktreeRecord;
    use opencode_codes::protocol_generated::types::{
        TextPart, TextPartInputTime, UserMessage, UserMessageModel, UserMessageTime,
    };
    use std::path::Path;

    fn record(slug: &str) -> WorktreeRecord {
        WorktreeRecord::new(Path::new("/repo"), slug).unwrap()
    }

    #[test]
    fn upsert_sets_first_active() {
        let mut s = Store::default();
        s.upsert_worktree(record("feat-a"));
        assert_eq!(s.active_worktree.as_deref(), Some("feat-a"));
        s.upsert_worktree(record("feat-b"));
        assert_eq!(s.active_worktree.as_deref(), Some("feat-a"));
    }

    #[test]
    fn remove_falls_back_active() {
        let mut s = Store::default();
        s.upsert_worktree(record("feat-a"));
        s.upsert_worktree(record("feat-b"));
        s.set_active("feat-b");
        assert!(s.remove_worktree("feat-b"));
        assert_eq!(s.active_worktree.as_deref(), Some("feat-a"));
    }

    #[test]
    fn set_active_rejects_unknown() {
        let mut s = Store::default();
        assert!(!s.set_active("nope"));
    }

    #[test]
    fn blocked_mirror_round_trip() {
        let mut s = Store::default();
        let a = TaskId::new();
        let b = TaskId::new();
        assert!(!s.is_blocked_slug("wt-a"));
        s.mark_blocked(a, "wt-a");
        s.mark_blocked(b, "wt-b");
        assert!(s.is_blocked_slug("wt-a"));
        assert!(s.is_blocked_slug("wt-b"));
        // Clearing one slug leaves the other alone.
        assert_eq!(s.clear_blocked_by_slug("wt-a"), vec![a]);
        assert!(!s.is_blocked_slug("wt-a"));
        assert!(s.is_blocked_slug("wt-b"));
        // Unknown slugs clear nothing, never crash.
        assert!(s.clear_blocked_by_slug("nope").is_empty());
    }

    #[test]
    fn set_messages_mirrors_transcript() {
        let mut s = Store::default();
        let info = Message::User(UserMessage {
            agent: "opencode".into(),
            format: None,
            id: "u1".into(),
            model: UserMessageModel {
                model_id: "m".into(),
                provider_id: "p".into(),
                variant: None,
            },
            role: "user".into(),
            session_id: "s1".into(),
            summary: None,
            system: None,
            time: UserMessageTime { created: 7.0 },
            tools: None,
        });
        let parts = vec![Part::Text(TextPart {
            id: "p1".into(),
            ignored: None,
            message_id: "u1".into(),
            metadata: None,
            session_id: "s1".into(),
            synthetic: None,
            text: "hello mirror".into(),
            time: Some(TextPartInputTime {
                end: None,
                start: 1,
            }),
            type_: "text".into(),
        })];
        s.set_messages("s1", vec![MessageEntry { info, parts }]);
        // Legacy still authoritative …
        assert_eq!(s.messages.get("s1").unwrap().len(), 1);
        // … and the unified mirror follows.
        let t = s.transcript_for_session("s1");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].role, crate::transcript::Role::User);
        assert_eq!(t[0].text, "hello mirror");
        // Stable mapping: second write lands in the same task.
        let task = t[0].task;
        assert_eq!(s.task_for_session("s1"), task);
    }

    #[test]
    fn retire_drops_mirror_pending_and_totals() {
        let mut s = Store::default();
        let task = s.task_for_session("s1");
        s.push_unified(UnifiedMessage {
            id: "m1".into(),
            task,
            role: crate::transcript::Role::Agent,
            text: "x".into(),
            tool: None,
            ts: 0,
        });
        s.costs.insert(
            task,
            Cost {
                input: 1.0,
                output: 0.0,
                cache: 0.0,
                cost: 0.1,
            },
        );
        s.pending_permissions.insert(
            "p1".into(),
            PendingPermission {
                permission_id: "p1".into(),
                session_id: "s1".into(),
                ..Default::default()
            },
        );
        s.totals.input = 99.0;
        s.retire_session("s1");
        assert!(!s.transcripts.contains_key(&task));
        assert!(!s.costs.contains_key(&task));
        assert!(!s.session_task.contains_key("s1"));
        assert!(s.pending_permissions.is_empty());
        // Totals recomputed over the remaining (empty) messages.
        assert_eq!(s.totals.input, 0.0);
    }
}
