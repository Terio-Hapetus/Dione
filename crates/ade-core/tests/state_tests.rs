//! State-machine tests driven by real opencode wire JSON.
//! Fast, deterministic, no network.

use ade_core::Store;
use opencode_codes::protocol_generated::types::{Event, SessionStatus};
use serde_json::json;

fn session_json(id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "directory": "/repo",
        "projectID": "proj_1",
        "slug": "test",
        "time": {"created": 1, "updated": 2},
        "title": "test session",
        "version": "1"
    })
}

fn apply(store: &mut Store, v: serde_json::Value) {
    let ev: Event = serde_json::from_value(v).expect("event must decode");
    store.apply_event(&ev);
}

#[test]
fn session_created_becomes_active() {
    let mut s = Store::default();
    apply(
        &mut s,
        json!({
            "type": "session.created",
            "id": "evt_1",
            "properties": {"info": session_json("ses_1"), "sessionID": "ses_1"}
        }),
    );
    assert_eq!(s.active_session.as_deref(), Some("ses_1"));
    assert_eq!(s.sessions["ses_1"].title, "test session");
}

#[test]
fn status_busy_marks_busy() {
    let mut s = Store {
        active_session: Some("ses_1".into()),
        ..Default::default()
    };
    apply(
        &mut s,
        json!({
            "type": "session.status",
            "id": "evt_2",
            "properties": {"sessionID": "ses_1", "status": {"type": "busy"}}
        }),
    );
    assert!(matches!(s.statuses["ses_1"], SessionStatus::Busy));
    assert!(s.is_busy());
}

#[test]
fn session_idle_clears_busy() {
    let mut s = Store {
        active_session: Some("ses_1".into()),
        ..Default::default()
    };
    s.statuses.insert("ses_1".into(), SessionStatus::Busy);
    apply(
        &mut s,
        json!({"type": "session.idle", "id": "evt_3", "properties": {"sessionID": "ses_1"}}),
    );
    assert!(matches!(s.statuses["ses_1"], SessionStatus::Idle));
    assert!(!s.is_busy());
}

#[test]
fn permission_asked_collects_pending() {
    let mut s = Store::default();
    apply(
        &mut s,
        json!({
            "type": "permission.asked",
            "id": "evt_4",
            "properties": {
                "id": "per_1",
                "sessionID": "ses_1",
                "permission": "bash",
                "patterns": ["cargo *"],
                "metadata": {"command": "cargo test"},
                "always": []
            }
        }),
    );
    let p = &s.pending_permissions["per_1"];
    assert_eq!(p.session_id, "ses_1");
    assert_eq!(p.kind, "bash");
    assert_eq!(p.patterns, vec!["cargo *".to_string()]);
}

#[test]
fn permission_replied_clears_pending() {
    let mut s = Store::default();
    apply(
        &mut s,
        json!({
            "type": "permission.asked",
            "id": "evt_4",
            "properties": {
                "id": "per_1",
                "sessionID": "ses_1",
                "permission": "bash",
                "patterns": [],
                "metadata": {},
                "always": []
            }
        }),
    );
    apply(
        &mut s,
        json!({
            "type": "permission.replied",
            "id": "evt_5",
            "properties": {"requestID": "per_1", "sessionID": "ses_1", "reply": "once"}
        }),
    );
    assert!(s.pending_permissions.is_empty());
}

#[test]
fn message_updated_upserts_info() {
    let mut s = Store::default();
    let user = json!({
        "role": "user",
        "id": "msg_1",
        "agent": "opencode",
        "model": {"modelID": "m", "providerID": "p"},
        "sessionID": "ses_1",
        "time": {"created": 1.0}
    });
    apply(
        &mut s,
        json!({
            "type": "message.updated",
            "id": "evt_6",
            "properties": {"info": user, "sessionID": "ses_1"}
        }),
    );
    assert_eq!(s.messages["ses_1"].len(), 1);
    // Second update for the same id replaces instead of duplicating.
    apply(
        &mut s,
        json!({
            "type": "message.updated",
            "id": "evt_7",
            "properties": {"info": user, "sessionID": "ses_1"}
        }),
    );
    assert_eq!(s.messages["ses_1"].len(), 1);
}

#[test]
fn todo_updated_stores_todos() {
    let mut s = Store::default();
    apply(
        &mut s,
        json!({
            "type": "todo.updated",
            "id": "evt_8",
            "properties": {
                "sessionID": "ses_1",
                "todos": [{"content": "write tests", "priority": "high", "status": "in_progress"}]
            }
        }),
    );
    assert_eq!(s.todos["ses_1"][0].content, "write tests");
}

#[test]
fn session_deleted_falls_back_active() {
    let mut s = Store::default();
    for id in ["ses_1", "ses_2"] {
        apply(
            &mut s,
            json!({
                "type": "session.created",
                "id": format!("evt_{id}"),
                "properties": {"info": session_json(id), "sessionID": id}
            }),
        );
    }
    assert_eq!(s.active_session.as_deref(), Some("ses_1"));
    apply(
        &mut s,
        json!({
            "type": "session.deleted",
            "id": "evt_del",
            "properties": {"info": session_json("ses_1"), "sessionID": "ses_1"}
        }),
    );
    assert!(!s.sessions.contains_key("ses_1"));
    assert_eq!(s.active_session.as_deref(), Some("ses_2"));
}

#[test]
fn retired_session_frames_are_dropped() {
    use ade_core::state::event_session_id;

    let mut s = Store::default();
    apply(
        &mut s,
        json!({
            "type": "session.created",
            "id": "evt_1",
            "properties": {"info": session_json("ses_1"), "sessionID": "ses_1"}
        }),
    );
    s.session_scope.insert("ses_1".into(), "feat-a".into());
    s.retire_session("ses_1");
    assert!(!s.sessions.contains_key("ses_1"));

    // A late status frame must not resurrect anything.
    apply(
        &mut s,
        json!({
            "type": "session.status",
            "id": "evt_2",
            "properties": {"sessionID": "ses_1", "status": {"type": "busy"}}
        }),
    );
    assert!(!s.statuses.contains_key("ses_1"));

    // event_session_id covers the variants the pumps route on.
    let ev: Event = serde_json::from_value(json!({
        "type": "permission.asked",
        "id": "evt_3",
        "properties": {
            "id": "per_9", "sessionID": "ses_9", "permission": "bash",
            "patterns": [], "metadata": {}, "always": []
        }
    }))
    .unwrap();
    assert_eq!(event_session_id(&ev), Some("ses_9"));
}

#[test]
fn worktree_status_derives_from_session() {
    use ade_core::WorktreeStatus;
    use std::path::Path;

    let mut s = Store::default();
    let mut r = ade_core::worktree::WorktreeRecord::new(Path::new("/repo"), "feat-a").unwrap();
    // No session yet -> Creating.
    s.worktrees.insert(r.slug.clone(), r.clone());
    assert_eq!(s.worktree_status("feat-a"), WorktreeStatus::Creating);

    // Link a session: no messages, idle -> still Creating.
    r.session_id = Some("ses_1".into());
    s.worktrees.insert(r.slug.clone(), r);
    s.session_scope.insert("ses_1".into(), "feat-a".into());
    apply(
        &mut s,
        json!({
            "type": "session.created",
            "id": "evt_1",
            "properties": {"info": session_json("ses_1"), "sessionID": "ses_1"}
        }),
    );
    assert_eq!(s.worktree_status("feat-a"), WorktreeStatus::Creating);

    // Busy -> Working.
    s.statuses.insert("ses_1".into(), SessionStatus::Busy);
    assert_eq!(s.worktree_status("feat-a"), WorktreeStatus::Working);

    // Pending permission beats busy -> NeedsYou.
    apply(
        &mut s,
        json!({
            "type": "permission.asked",
            "id": "evt_2",
            "properties": {
                "id": "per_1", "sessionID": "ses_1", "permission": "bash",
                "patterns": [], "metadata": {}, "always": []
            }
        }),
    );
    assert_eq!(s.worktree_status("feat-a"), WorktreeStatus::NeedsYou);
}

#[test]
fn sessions_group_by_scope() {
    let mut s = Store::default();
    for id in ["ses_1", "ses_2", "ses_3"] {
        apply(
            &mut s,
            json!({
                "type": "session.created",
                "id": format!("evt_{id}"),
                "properties": {"info": session_json(id), "sessionID": id}
            }),
        );
    }
    s.session_scope.insert("ses_1".into(), "feat-a".into());
    s.session_scope.insert("ses_2".into(), "feat-a".into());
    // ses_3 stays in root scope.
    assert_eq!(s.sessions_in_scope("feat-a").len(), 2);
    assert_eq!(s.sessions_in_scope(""), vec!["ses_3".to_string()]);
    assert_eq!(s.scope_of("ses_9"), "");
}

#[test]
fn format_review_notes_renders_items() {
    use ade_core::state::{DiffNote, format_review_notes, resolve_range};

    let notes = vec![
        DiffNote {
            session_id: "ses_1".into(),
            file: "a.rs".into(),
            line: 12,
            end_line: None,
            text: "rename this".into(),
            replies: vec!["sure, done".into()],
        },
        DiffNote {
            session_id: "ses_1".into(),
            file: "b.rs".into(),
            line: 3,
            end_line: Some(7),
            text: "add test".into(),
            replies: Vec::new(),
        },
        DiffNote {
            session_id: "ses_1".into(),
            file: "c.rs".into(),
            line: 5,
            end_line: Some(5),
            text: "single".into(),
            replies: Vec::new(),
        },
    ];
    let body = format_review_notes(&notes);
    assert!(body.contains("a.rs:12 — rename this"));
    assert!(body.contains("↳ sure, done"));
    assert!(body.contains("b.rs:3-7 — add test"));
    assert!(body.contains("c.rs:5 — single"));

    // Two-click range machine: first click anchors, second resolves.
    let click = ("s".to_string(), "f".to_string(), 12u32);
    let (anchor, range) = resolve_range(None, click.clone());
    assert_eq!(anchor, Some(click.clone()));
    assert_eq!(range, None);
    // Second click below → range; order normalized either way.
    let (anchor2, range2) =
        resolve_range(anchor.clone(), ("s".to_string(), "f".to_string(), 18u32));
    assert_eq!(anchor2, None);
    assert_eq!(range2, Some((12, 18)));
    let (_, range3) = resolve_range(anchor, ("s".to_string(), "f".to_string(), 8u32));
    assert_eq!(range3, Some((8, 12)));
    // Clicking the anchor line itself → single line, anchor cleared.
    let (_, range4) = resolve_range(
        Some(("s".to_string(), "f".to_string(), 12u32)),
        ("s".to_string(), "f".to_string(), 12u32),
    );
    assert_eq!(range4, Some((12, 12)));
    // Different file/session moves the anchor instead of ranging.
    let (anchor5, range5) = resolve_range(
        Some(("s".to_string(), "f".to_string(), 12u32)),
        ("s".to_string(), "g".to_string(), 20u32),
    );
    assert_eq!(range5, None);
    assert_eq!(anchor5, Some(("s".to_string(), "g".to_string(), 20u32)));
}

#[test]
fn append_reply_targets_note_by_value() {
    use ade_core::state::{DiffNote, append_reply};

    let mut notes = vec![DiffNote {
        session_id: "s".into(),
        file: "a.rs".into(),
        line: 1,
        end_line: None,
        text: "fix".into(),
        replies: Vec::new(),
    }];
    let target = notes[0].clone();
    assert!(append_reply(&mut notes, &target, "on it".into()));
    assert_eq!(notes[0].replies, vec!["on it".to_string()]);
    // Stale target (note edited/deleted meanwhile) fails cleanly.
    assert!(!append_reply(&mut notes, &target, "late".into()));
    assert!(!append_reply(&mut [], &target, "empty".into()));
}

#[test]
fn parse_patch_lines_multi_hunk() {
    use ade_core::state::parse_patch_lines;

    let patch = "--- a/f.rs\n+++ b/f.rs\n@@ -1,3 +1,4 @@\n ctx\n-old\n+new1\n+new2\n ctx2\n@@ -10,2 +11,2 @@\n-old2\n+new3\n";
    let lines = parse_patch_lines(patch);
    let numbered: Vec<(Option<u32>, &str)> =
        lines.iter().map(|l| (l.line, l.text.as_str())).collect();
    assert!(
        numbered
            .iter()
            .any(|(n, t)| n.is_none() && t.starts_with("@@"))
    );
    // First hunk: ctx=1, -old=old-file 2, +new1=2, +new2=3, ctx2=4.
    let texts: Vec<&str> = numbered.iter().map(|(_, t)| *t).collect();
    let at = |t: &str| numbered.iter().find(|(_, x)| *x == t).unwrap().0;
    assert_eq!(at(" ctx"), Some(1));
    assert_eq!(at("-old"), Some(2));
    assert_eq!(at("+new1"), Some(2));
    assert_eq!(at("+new2"), Some(3));
    assert_eq!(at(" ctx2"), Some(4));
    // Second hunk restarts counters: -old2=old 10, +new3=new 11.
    assert_eq!(at("-old2"), Some(10));
    assert_eq!(at("+new3"), Some(11));
    assert!(texts.contains(&"--- a/f.rs"));
}

fn session_with_updated(id: &str, updated: u64) -> serde_json::Value {
    json!({
        "id": id,
        "directory": "/repo",
        "projectID": "proj_1",
        "slug": "test",
        "time": {"created": 1, "updated": updated},
        "title": "test session",
        "version": "1"
    })
}

fn create_review_session(s: &mut Store, id: &str, updated: u64, scope: &str) {
    apply(
        s,
        json!({
            "type": "session.created",
            "id": format!("evt-{id}"),
            "properties": {"info": session_with_updated(id, updated), "sessionID": id}
        }),
    );
    if !scope.is_empty() {
        s.session_scope.insert(id.to_string(), scope.to_string());
    }
    s.diffs.insert(id.to_string(), json!([]));
}

fn ask_permission(s: &mut Store, pid: &str, sid: &str) {
    apply(
        s,
        json!({
            "type": "permission.asked",
            "id": format!("evt-{pid}"),
            "properties": {
                "id": pid,
                "sessionID": sid,
                "permission": "bash",
                "patterns": [],
                "metadata": {},
                "always": []
            }
        }),
    );
}

#[test]
fn review_queue_needs_you_first_then_oldest() {
    let mut s = Store::default();
    create_review_session(&mut s, "ses_new", 300, "wt-a");
    create_review_session(&mut s, "ses_old", 100, "wt-b");
    create_review_session(&mut s, "ses_mid", 200, "");
    // Baseline: oldest activity first.
    assert_eq!(
        s.sort_review_sids(vec!["ses_new".into(), "ses_old".into(), "ses_mid".into()]),
        vec![
            "ses_old".to_string(),
            "ses_mid".to_string(),
            "ses_new".to_string()
        ]
    );
    // A pending permission jumps its session to the front.
    ask_permission(&mut s, "per_1", "ses_new");
    let sorted = s.sort_review_sids(vec!["ses_new".into(), "ses_old".into(), "ses_mid".into()]);
    assert_eq!(
        sorted,
        vec![
            "ses_new".to_string(),
            "ses_old".to_string(),
            "ses_mid".to_string()
        ]
    );
    // Scopes queue by their longest waiter — main competes on merit.
    assert_eq!(
        s.sort_review_scopes(vec!["".into(), "wt-a".into(), "wt-b".into()]),
        vec!["wt-a".to_string(), "wt-b".to_string(), "".to_string()]
    );
}

#[test]
fn review_queue_sinks_unknown_sessions() {
    let mut s = Store::default();
    create_review_session(&mut s, "ses_known", 50, "");
    let sorted = s.sort_review_sids(vec!["ses_ghost".into(), "ses_known".into()]);
    assert_eq!(
        sorted,
        vec!["ses_known".to_string(), "ses_ghost".to_string()]
    );
}

#[test]
fn parse_patch_lines_ignores_later_file_headers() {
    use ade_core::state::parse_patch_lines;

    let patch = "@@ -1,1 +1,1 @@\n-a\n+b\ndiff --git a/g.rs b/g.rs\nindex 123..456 100644\n--- a/g.rs\n+++ b/g.rs\n@@ -5,1 +5,1 @@\n-x\n+y\nnew file mode 100644\nBinary files a/i.png and b/i.png differ\n";
    let lines = parse_patch_lines(patch);
    let numbered: Vec<(Option<u32>, &str)> =
        lines.iter().map(|l| (l.line, l.text.as_str())).collect();
    let at = |t: &str| numbered.iter().find(|(_, x)| *x == t).unwrap().0;
    // File-boundary lines are never numbered, even mid-stream.
    for h in [
        "diff --git a/g.rs b/g.rs",
        "index 123..456 100644",
        "--- a/g.rs",
        "+++ b/g.rs",
        "new file mode 100644",
        "Binary files a/i.png and b/i.png differ",
    ] {
        assert_eq!(at(h), None, "{h} must not be numbered");
    }
    // Content numbering still works on both sides of the boundary.
    assert_eq!(at("-a"), Some(1));
    assert_eq!(at("+b"), Some(1));
    assert_eq!(at("-x"), Some(5));
    assert_eq!(at("+y"), Some(5));
}
