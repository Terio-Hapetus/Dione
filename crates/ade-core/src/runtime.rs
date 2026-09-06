//! M2 runtime: one `opencode serve`, one client per directory.
//!
//! UI thread sends [`Command`]s; the loop mutates a [`Store`] and publishes
//! `Arc<Store>` snapshots. SSE is best-effort — every `Connected` frame and
//! every poll tick reconciles via REST. Each worktree gets a directory-scoped
//! client plus its own SSE pump; routing is by `Store::session_scope`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use opencode_codes::client_async::OpencodeClient;
use opencode_codes::protocol_generated::types::{
    Event, PermissionReplyParams, PermissionReplyResponse, PromptAsyncParams,
    PromptAsyncParamsPartsItem, Session, SessionCreateParams, SessionStatus, SubtaskPartInputModel,
    TextPartInput, Todo,
};
use opencode_codes::sse::{RetryConfig, StreamEvent};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::config::AppConfig;
use crate::server::AdeServer;
use crate::state::{ConnState, MessageEntry, SelectedModel, Store};
use crate::worktree::{self, WorktreeRecord};

/// Scope key for the repo root (no worktree).
const ROOT_SCOPE: &str = "";

#[derive(Debug, Clone)]
pub enum Command {
    CreateSession {
        title: String,
    },
    SelectSession {
        id: String,
    },
    CreateWorktree {
        slug: String,
    },
    RemoveWorktree {
        slug: String,
    },
    MergeWorktree {
        slug: String,
    },
    SelectWorktree {
        slug: String,
    },
    Prompt {
        text: String,
    },
    FanOut {
        text: String,
    },
    Abort,
    FetchDiff(String),
    FetchAllDiffs,
    SendNotes {
        session_id: String,
        notes: Vec<crate::state::DiffNote>,
    },
    PermissionReply {
        permission_id: String,
        response: PermissionResponse,
    },
    SetModel {
        provider_id: String,
        model_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionResponse {
    Once,
    Always,
    Reject,
}

impl PermissionResponse {
    pub fn as_wire(&self) -> PermissionReplyResponse {
        match self {
            Self::Once => PermissionReplyResponse::Once,
            Self::Always => PermissionReplyResponse::Always,
            Self::Reject => PermissionReplyResponse::Reject,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeHandle {
    tx: UnboundedSender<Command>,
    slot: Arc<RwLock<Arc<Store>>>,
}

impl RuntimeHandle {
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn snapshot(&self) -> Arc<Store> {
        self.slot.read().map(|g| Arc::clone(&g)).unwrap_or_default()
    }
}

pub fn spawn(config: AppConfig) -> RuntimeHandle {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let slot: Arc<RwLock<Arc<Store>>> = Arc::new(RwLock::new(Arc::new(Store::default())));
    let handle = RuntimeHandle {
        tx,
        slot: Arc::clone(&slot),
    };

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime for ADE");
        rt.block_on(outer_loop(config, rx, slot));
    });

    handle
}

async fn outer_loop(
    config: AppConfig,
    mut rx: UnboundedReceiver<Command>,
    slot: Arc<RwLock<Arc<Store>>>,
) {
    loop {
        match AdeServer::start(&config).await {
            Ok(server) => {
                {
                    let mut st = LoopState::load(&slot);
                    st.store.conn = ConnState::Connected;
                    publish(&slot, &st);
                }
                run_session(&config, server, &mut rx, &slot).await;
                // run_session only returns on fatal stream/setup failure: retry.
                let mut st = LoopState::load(&slot);
                st.store.conn = ConnState::Disconnected;
                st.store.push_error("server loop ended — reconnecting");
                publish(&slot, &st);
            }
            Err(e) => {
                let mut st = LoopState::load(&slot);
                st.store.conn = ConnState::Disconnected;
                st.store
                    .push_error(format!("opencode serve failed to start: {e:#}"));
                publish(&slot, &st);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

struct LoopState {
    store: Store,
    base_url: String,
    repo: PathBuf,
    /// Scope ("" = root, else worktree slug) -> directory-scoped client.
    clients: BTreeMap<String, OpencodeClient>,
    pumped: BTreeSet<String>,
}

impl LoopState {
    fn load(slot: &RwLock<Arc<Store>>) -> Self {
        let store = slot.read().map(|g| (**g).clone()).unwrap_or_default();
        Self {
            store,
            base_url: String::new(),
            repo: PathBuf::from("."),
            clients: BTreeMap::new(),
            pumped: BTreeSet::new(),
        }
    }

    fn client_for(&self, sid: &str) -> &OpencodeClient {
        let scope = self.store.scope_of(sid);
        self.clients
            .get(scope)
            .or_else(|| self.clients.get(ROOT_SCOPE))
            .expect("root client always present")
    }
}

fn build_client(base_url: &str, dir: Option<&std::path::Path>) -> anyhow::Result<OpencodeClient> {
    let mut b = OpencodeClient::builder()
        .base_url(base_url)
        .auth_from_env()
        .timeout(Duration::from_secs(60));
    if let Some(d) = dir {
        b = b.directory(d.to_string_lossy().into_owned());
    }
    Ok(b.build()?)
}

/// Get (or lazily build + pump) the client for a scope.
fn ensure_client(
    st: &mut LoopState,
    slot: &Arc<RwLock<Arc<Store>>>,
    scope: &str,
) -> anyhow::Result<OpencodeClient> {
    if let Some(c) = st.clients.get(scope) {
        return Ok(c.clone());
    }
    let dir = if scope.is_empty() {
        None
    } else {
        Some(worktree::worktree_path(&st.repo, scope))
    };
    let client = build_client(&st.base_url, dir.as_deref())?;
    st.clients.insert(scope.to_string(), client.clone());
    spawn_pump(
        client.clone(),
        Arc::clone(slot),
        scope.to_string(),
        &mut st.pumped,
    );
    Ok(client)
}

fn spawn_pump(
    client: OpencodeClient,
    slot: Arc<RwLock<Arc<Store>>>,
    scope: String,
    pumped: &mut BTreeSet<String>,
) {
    if !pumped.insert(scope.clone()) {
        return;
    }
    tokio::spawn(async move {
        sse_pump(client, slot, scope).await;
    });
}

fn publish(slot: &RwLock<Arc<Store>>, st: &LoopState) {
    if let Ok(mut guard) = slot.write() {
        *guard = Arc::new(st.store.clone());
    }
}

async fn run_session(
    config: &AppConfig,
    server: AdeServer,
    rx: &mut UnboundedReceiver<Command>,
    slot: &Arc<RwLock<Arc<Store>>>,
) {
    let client = server.client.clone();
    let mut st = LoopState::load(slot);
    st.base_url = server.base_url.clone();
    st.repo = config.project_dir.clone();
    st.clients.insert(ROOT_SCOPE.to_string(), client.clone());
    spawn_pump(
        client.clone(),
        Arc::clone(slot),
        ROOT_SCOPE.to_string(),
        &mut st.pumped,
    );

    bootstrap(&mut st).await;
    publish(slot, &st);

    let mut poll = tokio::time::interval(Duration::from_millis(config.poll_interval_ms));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut tick: u64 = 0;

    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { return }; // UI gone: end session loop.
                if !handle_command(&mut st, slot, cmd).await {
                    return;
                }
                publish(slot, &st);
            }
            _ = poll.tick() => {
                tick += 1;
                poll_once(&mut st, tick).await;
                publish(slot, &st);
            }
        }
    }
}

async fn sse_pump(client: OpencodeClient, slot: Arc<RwLock<Arc<Store>>>, scope: String) {
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
                let scopes: Vec<String> = st
                    .store
                    .session_scope
                    .iter()
                    .filter(|(_, s)| *s == &scope)
                    .map(|(id, _)| id.clone())
                    .collect();
                for sid in scopes {
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
    st.store.conn = ConnState::Disconnected;
    st.store.push_error("event stream ended");
    publish(&slot, &st);
}

async fn apply_stream_event(
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

async fn poll_once(st: &mut LoopState, tick: u64) {
    reconcile_all_sessions(st).await;
    let active = st.store.active_session.clone();
    if let Some(sid) = active {
        let client = st.client_for(&sid).clone();
        reconcile_messages(st, &client, &sid).await;
        fetch_todos(st, &client, &sid).await;
    }
    if tick.is_multiple_of(10) {
        let root = st.clients.get(ROOT_SCOPE).cloned();
        if let Some(client) = root {
            fetch_providers(st, &client).await;
        }
    }
}

/// Returns false when the session loop should end (currently never — all
/// commands are recoverable).
async fn handle_command(st: &mut LoopState, slot: &Arc<RwLock<Arc<Store>>>, cmd: Command) -> bool {
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

async fn create_session_in(
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

async fn create_worktree(st: &mut LoopState, slot: &Arc<RwLock<Arc<Store>>>, slug: String) {
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
fn drop_scope(st: &mut LoopState, slug: &str, sids: &[String]) {
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

fn scoped_sessions(st: &LoopState, slug: &str) -> Vec<String> {
    st.store
        .session_scope
        .iter()
        .filter(|(_, s)| *s == slug)
        .map(|(id, _)| id.clone())
        .collect()
}

async fn remove_worktree(st: &mut LoopState, slug: &str) {
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

async fn merge_worktree(st: &mut LoopState, slug: &str) {
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

async fn select_worktree(st: &mut LoopState, slug: &str) {
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
async fn fan_out(st: &mut LoopState, text: String) {
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

async fn fetch_diff(st: &mut LoopState, client: &OpencodeClient, sid: &str) {
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

async fn prompt_session(st: &mut LoopState, client: &OpencodeClient, sid: &str, text: String) {
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

async fn reply_permission(
    st: &mut LoopState,
    client: &OpencodeClient,
    permission_id: String,
    response: PermissionResponse,
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
            .push_error(format!("permission reply failed: {e:#}")),
    }
}

async fn reconcile_messages(st: &mut LoopState, client: &OpencodeClient, sid: &str) {
    match client.list_messages(sid).await {
        Ok(msgs) => {
            st.store
                .set_messages(sid, msgs.into_iter().map(MessageEntry::from).collect());
        }
        Err(e) => st.store.push_error(format!("list messages failed: {e:#}")),
    }
}

async fn reconcile_scoped_sessions(st: &mut LoopState, client: &OpencodeClient, scope: &str) {
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

async fn reconcile_all_sessions(st: &mut LoopState) {
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

async fn fetch_todos(st: &mut LoopState, client: &OpencodeClient, sid: &str) {
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

async fn bootstrap(st: &mut LoopState) {
    discover_worktrees(st).await;
    reconcile_all_sessions(st).await;
    if let Some(root) = st.clients.get(ROOT_SCOPE).cloned() {
        fetch_providers(st, &root).await;
    }
}
/// Adopt on-disk managed worktrees (e.g. from a previous run) into the store.
async fn discover_worktrees(st: &mut LoopState) {
    let infos = worktree::list(&st.repo).await.unwrap_or_default();
    for info in infos {
        if !worktree::is_worktree_path(&info.path) {
            continue;
        }
        let Some(slug) = info
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if st.store.worktrees.contains_key(&slug) {
            continue;
        }
        let branch = info.branch.unwrap_or_else(|| worktree::branch_name(&slug));
        st.store.upsert_worktree(WorktreeRecord {
            slug,
            branch,
            path: info.path,
            status: crate::worktree::WorktreeStatus::Creating,
            session_id: None,
        });
    }
}

/// Parse `GET /provider` defensively — the shape is not in the wrapped spec.
async fn fetch_providers(st: &mut LoopState, client: &OpencodeClient) {
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

    let parsed: Result<Vec<crate::state::ProviderInfo>, anyhow::Error> = async {
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
                crate::state::ProviderInfo {
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
