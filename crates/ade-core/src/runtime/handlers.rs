use std::sync::{Arc, RwLock};

use opencode_codes::protocol_generated::types::SessionStatus;

use super::LoopState;
use super::commands::Command;
use super::io::{create_session_in, fetch_diff, prompt_session, reply_permission};
use super::reconcile::{fetch_todos, reconcile_messages};
use super::{ROOT_SCOPE, ensure_client};
use crate::state::SelectedModel;
use crate::worktree;

/// Returns false when the session loop should end (currently never — all
/// commands are recoverable).
pub(crate) async fn handle_command(
    st: &mut LoopState,
    slot: &Arc<RwLock<Arc<crate::state::Store>>>,
    cmd: Command,
) -> bool {
    match cmd {
        Command::CreateSession { title } => {
            let scope = st.store.active_worktree.clone().unwrap_or_default();
            match ensure_client(st, slot, &scope) {
                Err(e) => st
                    .store
                    .push_error(format!("worktree client failed: {e:#}")),
                Ok(client) => {
                    create_session_in(st, &client, &scope, title).await;
                }
            }
        }
        Command::CreateWorktree { slug } => create_worktree(st, slot, slug).await,
        Command::RemoveWorktree { slug } => remove_worktree(st, &slug).await,
        Command::MergeWorktree { slug } => merge_worktree(st, &slug).await,
        Command::SelectWorktree { slug } => select_worktree(st, &slug).await,
        Command::SelectSession { id } => {
            if st.store.sessions.contains_key(&id) {
                st.store.active_session = Some(id.clone());
                // Keep the worktree highlight in sync with the session.
                let scope = st.store.scope_of(&id).to_string();
                st.store.active_worktree = if scope.is_empty() { None } else { Some(scope) };
                let client = st.client_for(&id).clone();
                reconcile_messages(st, &client, &id).await;
                fetch_todos(st, &client, &id).await;
            }
        }
        Command::Prompt { text } => {
            let sid = st.store.active_session.clone();
            if let Some(sid) = sid {
                let client = st.client_for(&sid).clone();
                prompt_session(st, &client, &sid, text).await;
            } else {
                st.store.push_error("no active session — create one first");
            }
        }
        Command::FanOut { text } => fan_out(st, text).await,
        Command::SendNotes { session_id, notes } => {
            if notes.is_empty() {
                return true;
            }
            let client = st.client_for(&session_id).clone();
            let body = crate::state::format_review_notes(&notes);
            prompt_session(st, &client, &session_id, body).await;
        }
        Command::Abort => {
            if let Some(sid) = st.store.active_session.clone() {
                let client = st.client_for(&sid).clone();
                if let Err(e) = client.abort(&sid).await {
                    st.store.push_error(format!("abort failed: {e:#}"));
                }
            }
        }
        Command::PermissionReply {
            permission_id,
            response,
        } => {
            let sid = st
                .store
                .pending_permissions
                .get(&permission_id)
                .map(|p| p.session_id.clone());
            if let Some(sid) = sid {
                let client = st.client_for(&sid).clone();
                reply_permission(st, &client, permission_id, response).await;
            } else {
                st.store.push_error("reply for unknown permission");
            }
        }
        Command::FetchDiff(sid) => {
            let client = st.client_for(&sid).clone();
            fetch_diff(st, &client, &sid).await;
        }
        Command::FetchAllDiffs => {
            let sids: Vec<String> = st.store.sessions.keys().cloned().collect();
            for sid in sids {
                let client = st.client_for(&sid).clone();
                fetch_diff(st, &client, &sid).await;
            }
        }
        Command::SetModel {
            provider_id,
            model_id,
        } => {
            st.store.selected_model = Some(SelectedModel {
                provider_id,
                id: model_id,
            });
        }
    }
    true
}

pub(crate) async fn create_worktree(
    st: &mut LoopState,
    slot: &Arc<RwLock<Arc<crate::state::Store>>>,
    slug: String,
) {
    let record = match worktree::create(&st.repo, &slug).await {
        Ok(r) => r,
        Err(e) => {
            st.store
                .push_error(format!("create worktree failed: {e:#}"));
            return;
        }
    };
    let slug = record.slug.clone();
    st.store.upsert_worktree(record);
    st.store.active_worktree = Some(slug.clone());
    match ensure_client(st, slot, &slug) {
        Err(e) => st
            .store
            .push_error(format!("worktree client failed: {e:#}")),
        Ok(client) => {
            create_session_in(st, &client, &slug, format!("work in {slug}")).await;
        }
    }
}

/// Forget a worktree's sessions/clients/records after its git dir is gone
/// (removed or merged). Shared by remove and merge paths.
pub(crate) fn drop_scope(st: &mut LoopState, slug: &str, sids: &[String]) {
    for sid in sids {
        st.store.retire_session(sid);
    }
    st.store.remove_worktree(slug);
    st.clients.remove(slug);
    st.pumped.remove(slug);
    if st.store.active_worktree.as_deref() == Some(slug) {
        st.store.active_worktree = st.store.worktrees.keys().next().cloned();
    }
}

pub(crate) fn scoped_sessions(st: &LoopState, slug: &str) -> Vec<String> {
    st.store
        .session_scope
        .iter()
        .filter(|(_, s)| *s == slug)
        .map(|(id, _)| id.clone())
        .collect()
}

pub(crate) async fn remove_worktree(st: &mut LoopState, slug: &str) {
    // Abort + retire every session scoped to this worktree.
    let sids = scoped_sessions(st, slug);
    for sid in &sids {
        if let Some(client) = st
            .clients
            .get(slug)
            .or_else(|| st.clients.get(ROOT_SCOPE))
            .cloned()
        {
            let _ = client.abort(sid).await;
        }
    }
    if let Err(e) = worktree::remove(&st.repo, slug).await {
        st.store
            .push_error(format!("remove worktree failed: {e:#}"));
    }
    drop_scope(st, slug, &sids);
}

pub(crate) async fn merge_worktree(st: &mut LoopState, slug: &str) {
    let sids = scoped_sessions(st, slug);
    match worktree::merge_winner(&st.repo, slug).await {
        Ok(summary) => {
            tracing::info!("merged {slug}: {summary}");
            drop_scope(st, slug, &sids);
            let _ = worktree::prune(&st.repo).await;
        }
        Err(e) => {
            st.store.push_error(format!("merge {slug} failed: {e:#}"));
        }
    }
}

pub(crate) async fn select_worktree(st: &mut LoopState, slug: &str) {
    if !st.store.worktrees.contains_key(slug) {
        return;
    }
    st.store.active_worktree = Some(slug.to_string());
    let sid = st
        .store
        .worktrees
        .get(slug)
        .and_then(|r| r.session_id.clone());
    if let Some(sid) = sid
        && st.store.sessions.contains_key(&sid)
    {
        st.store.active_session = Some(sid.clone());
        let client = st.client_for(&sid).clone();
        reconcile_messages(st, &client, &sid).await;
        fetch_todos(st, &client, &sid).await;
    }
}

/// Send one prompt to every worktree-linked session, skipping busy ones.
pub(crate) async fn fan_out(st: &mut LoopState, text: String) {
    let targets: Vec<(String, String)> = st
        .store
        .session_scope
        .iter()
        .filter(|(_, scope)| !scope.is_empty())
        .map(|(id, scope)| (id.clone(), scope.clone()))
        .collect();
    if targets.is_empty() {
        st.store
            .push_error("fan-out needs at least one worktree session");
        return;
    }
    let mut sent = 0;
    for (sid, scope) in targets {
        if st
            .store
            .statuses
            .get(&sid)
            .is_some_and(|s| matches!(s, SessionStatus::Busy | SessionStatus::Retry { .. }))
        {
            st.store
                .push_error(format!("fan-out skipped {scope}: busy"));
            continue;
        }
        let client = st
            .clients
            .get(&scope)
            .or_else(|| st.clients.get(ROOT_SCOPE))
            .cloned()
            .expect("root client always present");
        prompt_session(st, &client, &sid, text.clone()).await;
        sent += 1;
    }
    tracing::info!("fan-out sent to {sent} worktree sessions");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Store;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    fn empty_state() -> LoopState {
        LoopState {
            store: Store::default(),
            base_url: String::new(),
            repo: PathBuf::from("."),
            clients: BTreeMap::new(),
            pumped: BTreeSet::new(),
            fleet: Vec::new(),
            sweeper: None,
        }
    }

    #[test]
    fn drop_scope_retires_sessions_and_clears_worktree() {
        let mut st = empty_state();
        st.store.session_scope.insert("s1".into(), "wt-a".into());
        st.store.session_scope.insert("s2".into(), "wt-a".into());
        st.store.active_worktree = Some("wt-a".into());
        st.pumped.insert("wt-a".into());
        drop_scope(&mut st, "wt-a", &["s1".into(), "s2".into()]);
        assert!(st.store.retired_sessions.contains("s1"));
        assert!(st.store.retired_sessions.contains("s2"));
        assert!(!st.store.session_scope.contains_key("s1"));
        assert!(st.pumped.is_empty());
        assert_ne!(st.store.active_worktree.as_deref(), Some("wt-a"));
    }

    #[test]
    fn scoped_sessions_filters_by_slug() {
        let mut st = empty_state();
        st.store.session_scope.insert("s1".into(), "wt-a".into());
        st.store.session_scope.insert("s2".into(), "wt-b".into());
        assert_eq!(scoped_sessions(&st, "wt-a"), vec!["s1".to_string()]);
    }
}
