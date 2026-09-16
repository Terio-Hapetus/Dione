use opencode_codes::protocol_generated::types::{Event, Message, Part, SessionStatus};

use super::store::Store;
use super::types::{MessageEntry, PendingPermission};

/// Apply one SSE event to the mirror. Best-effort: unknown or
/// partial frames are ignored — REST reconcile is authoritative.
/// Frames for retired sessions (removed worktrees) are dropped.
pub fn apply_event(store: &mut Store, ev: &Event) {
    if event_session_id(ev).is_some_and(|id| store.retired_sessions.contains(id)) {
        return;
    }
    match ev {
        Event::SessionCreated(e) => {
            let s = e.properties.info.clone();
            if store.active_session.is_none() {
                store.active_session = Some(s.id.clone());
            }
            store.sessions.insert(s.id.clone(), s);
        }
        Event::SessionUpdated(e) => {
            let s = e.properties.info.clone();
            store.sessions.insert(s.id.clone(), s);
        }
        Event::SessionDeleted(e) => {
            let id = &e.properties.session_id;
            store.sessions.remove(id);
            store.statuses.remove(id);
            store.messages.remove(id);
            // Mirror first: it reads the session_task mapping below.
            store.drop_session_mirror(id);
            store.session_task.remove(id);
            if store.active_session.as_deref() == Some(id) {
                store.active_session = store.sessions.keys().next().cloned();
            }
        }
        Event::SessionStatus(e) => {
            store
                .statuses
                .insert(e.properties.session_id.clone(), e.properties.status.clone());
        }
        Event::SessionIdle(e) => {
            store
                .statuses
                .insert(e.properties.session_id.clone(), SessionStatus::Idle);
        }
        Event::MessageUpdated(e) => {
            let sid = e.properties.session_id.clone();
            let info = e.properties.info.clone();
            {
                let entries = store.messages.entry(sid.clone()).or_default();
                let id = message_id(&info);
                match entries.iter_mut().find(|x| message_id(&x.info) == id) {
                    Some(x) => x.info = info.clone(),
                    None => entries.push(MessageEntry {
                        info: info.clone(),
                        parts: Vec::new(),
                    }),
                }
                store.recompute_totals();
            }
            let parts: Vec<Part> = store
                .messages
                .get(&sid)
                .and_then(|v| {
                    let id = message_id(&info);
                    v.iter().find(|x| message_id(&x.info) == id)
                })
                .map(|e| e.parts.clone())
                .unwrap_or_default();
            store.mirror_entry(&sid, &info, &parts);
        }
        Event::MessagePartUpdated(e) => {
            let sid = e.properties.session_id.clone();
            let part = e.properties.part.clone();
            {
                let entries = store.messages.entry(sid.clone()).or_default();
                if let Some((msg_id, part_id)) = part_key(&part)
                    && let Some(entry) = entries.iter_mut().find(|x| message_id(&x.info) == msg_id)
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
            let snapshot: Option<(Message, Vec<Part>)> = store.messages.get(&sid).and_then(|v| {
                part_key(&part).and_then(|(msg_id, _)| {
                    v.iter()
                        .find(|x| message_id(&x.info) == msg_id)
                        .map(|e| (e.info.clone(), e.parts.clone()))
                })
            });
            if let Some((info, parts)) = snapshot {
                store.mirror_entry(&sid, &info, &parts);
            }
        }
        Event::PermissionAsked(e) => {
            let p = &e.properties;
            store.pending_permissions.insert(
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
            store.pending_permissions.remove(&e.properties.request_id);
        }
        Event::TodoUpdated(e) => {
            store
                .todos
                .insert(e.properties.session_id.clone(), e.properties.todos.clone());
        }
        _ => {}
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

impl Store {
    /// Backward-compat method: `store.apply_event(&ev)`.
    /// New code can also call `events::apply_event(&mut store, &ev)`.
    pub fn apply_event(&mut self, ev: &Event) {
        apply_event(self, ev);
    }
}
