//! M1 store: worktrees (M0) + sessions/messages/permissions/diffs.
//! M2 links sessions to worktrees (`session_scope`) for the fleet dashboard.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use opencode_codes::protocol_generated::types::{
    Event, Message, MessageWithParts, Part, Session, SessionStatus, Todo,
};

use crate::transcript::{Cost, TaskId, UnifiedMessage, cost_of_message, entry_to_unified};
use crate::worktree::{WorktreeRecord, WorktreeStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnState {
    #[default]
    Connecting,
    Connected,
    Disconnected,
}

#[derive(Debug, Clone)]
pub struct MessageEntry {
    pub info: Message,
    pub parts: Vec<Part>,
}

impl From<MessageWithParts> for MessageEntry {
    fn from(m: MessageWithParts) -> Self {
        Self {
            info: m.info,
            parts: m.parts,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PendingPermission {
    pub permission_id: String,
    pub session_id: String,
    pub kind: String,
    pub patterns: Vec<String>,
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderInfo {
    pub provider_id: String,
    pub provider_name: String,
    pub models: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct SelectedModel {
    pub provider_id: String,
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffNote {
    pub session_id: String,
    pub file: String,
    pub line: u32,
    pub text: String,
}

/// Format review notes as a prompt body for the agent.
pub fn format_review_notes(notes: &[DiffNote]) -> String {
    let mut out = String::from("Review feedback — please address each item:\n");
    for n in notes {
        out.push_str(&format!("- {}:{} — {}\n", n.file, n.line, n.text));
    }
    out
}

/// One rendered diff line with its new/old-file number, if countable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchLine {
    /// 1-based line number (`+`/` ` lines: new file; `-` lines: old file).
    pub line: Option<u32>,
    pub text: String,
}

/// Map unified-diff lines to file line numbers. Handles multi-hunk patches;
/// `+++`/`---`/headers/no-newline markers get `None`.
pub fn parse_patch_lines(patch: &str) -> Vec<PatchLine> {
    let mut out = Vec::new();
    let mut old: u32 = 0;
    let mut new: u32 = 0;
    let mut in_hunk = false;
    for line in patch.lines() {
        if line.starts_with("@@") {
            for part in line.split_whitespace() {
                if let Some(v) = part.strip_prefix('-') {
                    old = v.split(',').next().unwrap_or("0").parse().unwrap_or(0);
                } else if let Some(v) = part.strip_prefix('+') {
                    new = v.split(',').next().unwrap_or("0").parse().unwrap_or(0);
                }
            }
            in_hunk = true;
            out.push(PatchLine {
                line: None,
                text: line.to_string(),
            });
        } else if !in_hunk || line.starts_with("+++") || line.starts_with("---") {
            out.push(PatchLine {
                line: None,
                text: line.to_string(),
            });
        } else if line.starts_with('+') {
            out.push(PatchLine {
                line: Some(new),
                text: line.to_string(),
            });
            new = new.saturating_add(1);
        } else if line.starts_with('-') {
            out.push(PatchLine {
                line: Some(old),
                text: line.to_string(),
            });
            old = old.saturating_add(1);
        } else if line.starts_with('\\') {
            out.push(PatchLine {
                line: None,
                text: line.to_string(),
            });
        } else {
            out.push(PatchLine {
                line: Some(new),
                text: line.to_string(),
            });
            old = old.saturating_add(1);
            new = new.saturating_add(1);
        }
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct Totals {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cost: f64,
}

impl Totals {
    pub fn total_context(&self) -> f64 {
        self.input + self.cache_read
    }
}

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

    fn mirror_entry(&mut self, sid: &str, info: &Message, parts: &[Part]) {
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

    /// Drop a session and everything mirrored for it; future SSE frames for
    /// it are ignored. Returns its scope, if known.
    pub fn retire_session(&mut self, sid: &str) -> Option<String> {
        self.sessions.remove(sid);
        self.statuses.remove(sid);
        self.messages.remove(sid);
        self.todos.remove(sid);
        self.diffs.remove(sid);
        self.pending_permissions.retain(|_, p| p.session_id != sid);
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

    fn recompute_totals(&mut self) {
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

    /// Apply one SSE event to the mirror. Best-effort: unknown or
    /// partial frames are ignored — REST reconcile is authoritative.
    /// Frames for retired sessions (removed worktrees) are dropped.
    pub fn apply_event(&mut self, ev: &Event) {
        if event_session_id(ev).is_some_and(|id| self.retired_sessions.contains(id)) {
            return;
        }
        match ev {
            Event::SessionCreated(e) => {
                let s = e.properties.info.clone();
                if self.active_session.is_none() {
                    self.active_session = Some(s.id.clone());
                }
                self.sessions.insert(s.id.clone(), s);
            }
            Event::SessionUpdated(e) => {
                let s = e.properties.info.clone();
                self.sessions.insert(s.id.clone(), s);
            }
            Event::SessionDeleted(e) => {
                let id = &e.properties.session_id;
                self.sessions.remove(id);
                self.statuses.remove(id);
                self.messages.remove(id);
                self.session_task.remove(id);
                if self.active_session.as_deref() == Some(id) {
                    self.active_session = self.sessions.keys().next().cloned();
                }
            }
            Event::SessionStatus(e) => {
                self.statuses
                    .insert(e.properties.session_id.clone(), e.properties.status.clone());
            }
            Event::SessionIdle(e) => {
                self.statuses
                    .insert(e.properties.session_id.clone(), SessionStatus::Idle);
            }
            Event::MessageUpdated(e) => {
                let sid = e.properties.session_id.clone();
                let info = e.properties.info.clone();
                {
                    let entries = self.messages.entry(sid.clone()).or_default();
                    let id = message_id(&info);
                    match entries.iter_mut().find(|x| message_id(&x.info) == id) {
                        Some(x) => x.info = info.clone(),
                        None => entries.push(MessageEntry {
                            info: info.clone(),
                            parts: Vec::new(),
                        }),
                    }
                    self.recompute_totals();
                }
                let parts: Vec<Part> = self
                    .messages
                    .get(&sid)
                    .and_then(|v| {
                        let id = message_id(&info);
                        v.iter().find(|x| message_id(&x.info) == id)
                    })
                    .map(|e| e.parts.clone())
                    .unwrap_or_default();
                self.mirror_entry(&sid, &info, &parts);
            }
            Event::MessagePartUpdated(e) => {
                let sid = e.properties.session_id.clone();
                let part = e.properties.part.clone();
                {
                    let entries = self.messages.entry(sid.clone()).or_default();
                    if let Some((msg_id, part_id)) = part_key(&part)
                        && let Some(entry) =
                            entries.iter_mut().find(|x| message_id(&x.info) == msg_id)
                    {
                        match entry
                            .parts
                            .iter_mut()
                            .find(|p| part_id_of(p) == Some(part_id.as_str()))
                        {
                            Some(p) => *p = part.clone(),
                            None => entry.parts.push(part.clone()),
                        }
                    }
                }
                let snapshot: Option<(Message, Vec<Part>)> =
                    self.messages.get(&sid).and_then(|v| {
                        part_key(&part).and_then(|(msg_id, _)| {
                            v.iter()
                                .find(|x| message_id(&x.info) == msg_id)
                                .map(|e| (e.info.clone(), e.parts.clone()))
                        })
                    });
                if let Some((info, parts)) = snapshot {
                    self.mirror_entry(&sid, &info, &parts);
                }
            }
            Event::PermissionAsked(e) => {
                let p = &e.properties;
                self.pending_permissions.insert(
                    p.id.clone(),
                    PendingPermission {
                        permission_id: p.id.clone(),
                        session_id: p.session_id.clone(),
                        kind: p.permission.clone(),
                        patterns: p.patterns.clone(),
                        metadata: p.metadata.clone(),
                    },
                );
            }
            Event::PermissionReplied(e) => {
                self.pending_permissions.remove(&e.properties.request_id);
            }
            Event::TodoUpdated(e) => {
                self.todos
                    .insert(e.properties.session_id.clone(), e.properties.todos.clone());
            }
            _ => {}
        }
    }
}

pub fn message_id(m: &Message) -> &str {
    match m {
        Message::User(u) => &u.id,
        Message::Assistant(a) => &a.id,
    }
}

/// Session id carried by an event, if any — used to scope pumps and to
/// drop frames for retired sessions.
pub fn event_session_id(ev: &Event) -> Option<&str> {
    match ev {
        Event::SessionCreated(e) => Some(&e.properties.info.id),
        Event::SessionUpdated(e) => Some(&e.properties.info.id),
        Event::SessionDeleted(e) => Some(&e.properties.session_id),
        Event::SessionStatus(e) => Some(&e.properties.session_id),
        Event::SessionIdle(e) => Some(&e.properties.session_id),
        Event::MessageUpdated(e) => Some(&e.properties.session_id),
        Event::MessagePartUpdated(e) => Some(&e.properties.session_id),
        Event::PermissionAsked(e) => Some(&e.properties.session_id),
        Event::PermissionReplied(e) => Some(&e.properties.session_id),
        Event::TodoUpdated(e) => Some(&e.properties.session_id),
        _ => None,
    }
}

/// `(message_id, part_id)` for part variants that carry both.
fn part_key(p: &Part) -> Option<(String, String)> {
    let (msg, id) = match p {
        Part::Text(t) => (t.message_id.clone(), t.id.clone()),
        Part::Reasoning(r) => (r.message_id.clone(), r.id.clone()),
        Part::File(f) => (f.message_id.clone(), f.id.clone()),
        Part::Tool(t) => (t.message_id.clone(), t.id.clone()),
        Part::StepStart(s) => (s.message_id.clone(), s.id.clone()),
        Part::StepFinish(s) => (s.message_id.clone(), s.id.clone()),
        Part::Patch(p) => (p.message_id.clone(), p.id.clone()),
        _ => return None,
    };
    Some((msg, id))
}

fn part_id_of(p: &Part) -> Option<&str> {
    match p {
        Part::Text(t) => Some(&t.id),
        Part::Reasoning(r) => Some(&r.id),
        Part::File(f) => Some(&f.id),
        Part::Tool(t) => Some(&t.id),
        Part::StepStart(s) => Some(&s.id),
        Part::StepFinish(s) => Some(&s.id),
        Part::Patch(p) => Some(&p.id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
