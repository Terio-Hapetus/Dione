use opencode_codes::client_async::OpencodeClient;
use opencode_codes::protocol_generated::types::{Event, SessionStatus};
use opencode_codes::sse::{RetryConfig, StreamEvent};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use super::LoopState;
use super::publish;
use super::reconcile::{reconcile_messages, reconcile_scoped_sessions};
use crate::state::Store;

pub(crate) async fn sse_pump(client: OpencodeClient, slot: Arc<RwLock<Arc<Store>>>, scope: String) {
    let retry = RetryConfig {
        initial_interval: Duration::from_millis(500),
        max_interval: Duration::from_secs(10),
        factor: 1.5,
        max_retries: None,
    };
    let mut stream = match client.event_stream(retry) {
        Ok(s) => s,
        Err(e) => {
            let mut st = LoopState::load(&slot);
            st.store.push_error(format!("SSE subscribe failed: {e:#}"));
            publish(&slot, &st);
            return;
        }
    };
    while let Some(item) = stream.next().await {
        match item {
            Ok(StreamEvent::Connected) => {
                // Reconcile every session in this pump's scope (usually one).
                let mut st = LoopState::load(&slot);
                for sid in sessions_for_scope(&st.store, &scope) {
                    reconcile_messages(&mut st, &client, &sid).await;
                }
                publish(&slot, &st);
            }
            Ok(StreamEvent::Event(ev)) => {
                apply_stream_event(&client, &slot, *ev, scope.clone()).await;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::debug!("SSE frame error: {e:#}");
            }
        }
    }
    // Stream ended (server gone): mark disconnected so outer loop reconnects.
    let mut st = LoopState::load(&slot);
    st.store.conn = crate::state::ConnState::Disconnected;
    st.store.push_error("event stream ended");
    publish(&slot, &st);
}

pub(crate) async fn apply_stream_event(
    client: &OpencodeClient,
    slot: &Arc<RwLock<Arc<Store>>>,
    ev: Event,
    scope: String,
) {
    let mut st = LoopState::load(slot);
    // Attribute newly-seen sessions to this pump's scope (reconcile fixes
    // any misattribution; the scope map is advisory for routing).
    if let Event::SessionCreated(e) = &ev {
        st.store
            .session_scope
            .entry(e.properties.info.id.clone())
            .or_insert_with(|| scope.clone());
    }
    // Permission + todo frames carry full data — no extra fetch needed.
    // Message frames only patch the mirror; the poll tick reconciles fully.
    st.store.apply_event(&ev);
    if matches!(
        ev,
        Event::SessionCreated(_) | Event::SessionDeleted(_) | Event::SessionIdle(_)
    ) {
        reconcile_scoped_sessions(&mut st, client, &scope).await;
    }
    publish(slot, &st);
}

/// Pure helper (testable): session ids in a pump scope.
/// Skips retired sessions; routing table is advisory.
pub(crate) fn sessions_for_scope(store: &Store, scope: &str) -> Vec<String> {
    store
        .session_scope
        .iter()
        .filter(|(id, s)| *s == scope && !store.retired_sessions.contains(*id))
        .map(|(id, _)| id.clone())
        .collect()
}

#[allow(dead_code)]
fn _assert_busy_matches(s: &SessionStatus) -> bool {
    matches!(s, SessionStatus::Busy | SessionStatus::Retry { .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_for_scope_skips_retired() {
        let mut store = Store::default();
        store.session_scope.insert("s1".into(), "wt-a".into());
        store.session_scope.insert("s2".into(), "wt-a".into());
        store.retired_sessions.insert("s2".into());
        let got = sessions_for_scope(&store, "wt-a");
        assert_eq!(got, vec!["s1".to_string()]);
    }
}
