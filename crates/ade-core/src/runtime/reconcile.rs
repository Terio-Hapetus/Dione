use opencode_codes::client_async::OpencodeClient;
use opencode_codes::protocol_generated::types::{Session, Todo};

use super::LoopState;
use crate::state::{MessageEntry, ProviderInfo};

pub(crate) async fn reconcile_messages(st: &mut LoopState, client: &OpencodeClient, sid: &str) {
    match client.list_messages(sid).await {
        Ok(msgs) => {
            st.store
                .set_messages(sid, msgs.into_iter().map(MessageEntry::from).collect());
        }
        Err(e) => st.store.push_error(format!("list messages failed: {e:#}")),
    }
}

pub(crate) async fn reconcile_scoped_sessions(
    st: &mut LoopState,
    client: &OpencodeClient,
    scope: &str,
) {
    match client
        .request::<Vec<Session>>(reqwest::Method::GET, "/session", None)
        .await
    {
        Ok(sessions) => {
            for s in sessions {
                st.store
                    .session_scope
                    .entry(s.id.clone())
                    .or_insert_with(|| scope.to_string());
                if st.store.active_session.is_none() {
                    st.store.active_session = Some(s.id.clone());
                }
                st.store.sessions.insert(s.id.clone(), s);
            }
        }
        Err(e) => tracing::debug!("session list refresh failed: {e:#}"),
    }
}

pub(crate) async fn reconcile_all_sessions(st: &mut LoopState) {
    // Clone (cheap) so the borrow checker is happy across awaits.
    let clients: Vec<(String, OpencodeClient)> = st
        .clients
        .iter()
        .map(|(s, c)| (s.clone(), c.clone()))
        .collect();
    for (scope, client) in clients {
        reconcile_scoped_sessions(st, &client, &scope).await;
    }
}

pub(crate) async fn fetch_todos(st: &mut LoopState, client: &OpencodeClient, sid: &str) {
    let path = format!("/session/{sid}/todo");
    match client
        .request::<Vec<Todo>>(reqwest::Method::GET, &path, None)
        .await
    {
        Ok(todos) => {
            st.store.todos.insert(sid.to_string(), todos);
        }
        Err(e) => tracing::debug!("todos unavailable: {e:#}"),
    }
}

/// Parse `GET /provider` defensively — the shape is not in the wrapped spec.
pub(crate) async fn fetch_providers(st: &mut LoopState, client: &OpencodeClient) {
    #[derive(serde::Deserialize)]
    struct ProviderRaw {
        id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        models: Option<serde_json::Value>,
    }

    #[derive(serde::Deserialize)]
    struct ProvidersEnvelope {
        #[serde(default)]
        providers: Vec<ProviderRaw>,
    }

    let parsed: Result<Vec<ProviderInfo>, anyhow::Error> = async {
        let raw: serde_json::Value = client
            .request(reqwest::Method::GET, "/provider", None)
            .await?;
        let list: Vec<ProviderRaw> = if raw.is_array() {
            serde_json::from_value(raw)?
        } else {
            serde_json::from_value::<ProvidersEnvelope>(raw)?.providers
        };
        Ok(list
            .into_iter()
            .map(|p| {
                let mut models = Vec::new();
                if let Some(obj) = p.models.as_ref().and_then(|m| m.as_object()) {
                    for (model_id, mv) in obj {
                        let name = mv
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or(model_id)
                            .to_string();
                        models.push((model_id.clone(), name));
                    }
                }
                ProviderInfo {
                    provider_id: p.id,
                    provider_name: p.name.unwrap_or_default(),
                    models,
                }
            })
            .collect())
    }
    .await;

    match parsed {
        Ok(providers) => st.store.providers = providers,
        Err(e) => st
            .store
            .push_error(format!("providers fetch failed: {e:#}")),
    }
}
