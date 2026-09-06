use opencode_codes::client_async::OpencodeClient;
use opencode_codes::protocol_generated::types::{
    PermissionReplyParams, PromptAsyncParams, PromptAsyncParamsPartsItem, SessionCreateParams,
    SessionStatus, SubtaskPartInputModel, TextPartInput,
};

use super::LoopState;
use super::reconcile::reconcile_messages;
use crate::worktree;

pub(crate) async fn create_session_in(
    st: &mut LoopState,
    client: &OpencodeClient,
    scope: &str,
    title: String,
) {
    let params = SessionCreateParams {
        agent: None,
        metadata: None,
        model: None,
        parent_id: None,
        permission: None,
        title: Some(title).filter(|t| !t.trim().is_empty()),
        workspace_id: None,
    };
    match client.create_session(&params).await {
        Ok(s) => {
            st.store
                .session_scope
                .insert(s.id.clone(), scope.to_string());
            st.store.active_session = Some(s.id.clone());
            st.store.sessions.insert(s.id.clone(), s.clone());
            if !scope.is_empty()
                && let Some(record) = st.store.worktrees.get_mut(scope)
                && record.session_id.is_none()
            {
                record.session_id = Some(s.id.clone());
            }
            reconcile_messages(st, client, &s.id).await;
        }
        Err(e) => st.store.push_error(format!("create session failed: {e:#}")),
    }
}

pub(crate) async fn fetch_diff(st: &mut LoopState, client: &OpencodeClient, sid: &str) {
    // M3c: git first (agent-agnostic) — resolve the session's checkout.
    let scope = st.store.scope_of(sid).to_string();
    let path = if scope.is_empty() {
        st.repo.clone()
    } else {
        worktree::worktree_path(&st.repo, &scope)
    };
    match worktree::git_diff(&path).await {
        Ok(d) => {
            st.store.diffs.insert(sid.to_string(), d.to_json());
            return;
        }
        Err(e) => {
            tracing::debug!("git diff failed for {sid}, falling back to /session/diff: {e}");
        }
    }
    // Legacy fallback: opencode wire (removed once the UI reads git diffs).
    let path = format!("/session/{sid}/diff");
    match client
        .request::<serde_json::Value>(reqwest::Method::GET, &path, None)
        .await
    {
        Ok(v) => {
            st.store.diffs.insert(sid.to_string(), v);
        }
        Err(e) => st.store.push_error(format!("diff fetch failed: {e:#}")),
    }
}

pub(crate) async fn prompt_session(
    st: &mut LoopState,
    client: &OpencodeClient,
    sid: &str,
    text: String,
) {
    let params = PromptAsyncParams {
        agent: None,
        format: None,
        message_id: None,
        model: st
            .store
            .selected_model
            .as_ref()
            .map(|m| SubtaskPartInputModel {
                provider_id: m.provider_id.clone(),
                model_id: m.id.clone(),
            }),
        no_reply: None,
        parts: vec![PromptAsyncParamsPartsItem::Text(TextPartInput {
            id: None,
            ignored: None,
            metadata: None,
            synthetic: None,
            text,
            time: None,
            type_: String::new(),
        })],
        system: None,
        tools: None,
        variant: None,
    };
    match client.prompt_async(sid, &params).await {
        Err(e) => {
            st.store.push_error(format!("prompt failed: {e:#}"));
        }
        Ok(()) => {
            // Optimistically mark busy; authoritative status arrives via SSE/poll.
            st.store
                .statuses
                .insert(sid.to_string(), SessionStatus::Busy);
        }
    }
}

pub(crate) async fn reply_permission(
    st: &mut LoopState,
    client: &OpencodeClient,
    permission_id: String,
    response: super::commands::PermissionResponse,
) {
    let pending = st.store.pending_permissions.get(&permission_id).cloned();
    let Some(pending) = pending else {
        st.store.push_error("reply for unknown permission");
        return;
    };
    let params = PermissionReplyParams {
        response: response.as_wire(),
    };
    match client
        .respond_permission(&pending.session_id, &permission_id, &params)
        .await
    {
        Ok(_) => {
            st.store.pending_permissions.remove(&permission_id);
        }
        Err(e) => st
            .store
            .push_error(format!(" permission reply failed: {e:#}")),
    }
}
