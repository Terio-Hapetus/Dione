//! M2 runtime: one `opencode serve`, one client per directory.
//!
//! UI thread sends [`Command`]s; the loop mutates a [`Store`] and publishes
//! `Arc<Store>` snapshots. SSE is best-effort — every `Connected` frame and
//! every poll tick reconciles via REST. Each worktree gets a directory-scoped
//! client plus its own SSE pump; routing is by `Store::session_scope`.
//!
//! Facade: command types in `commands.rs`, session loop in `session.rs`,
//! SSE pump in `sse.rs`, REST reconcile in `reconcile.rs`, command handlers
//! in `handlers.rs`, wire verbs in `io.rs`. Public paths unchanged.

pub mod commands;
pub mod fleet;
pub mod handlers;
pub mod io;
pub mod reconcile;
pub mod session;
pub mod sse;

pub use commands::{Command, PermissionResponse};
pub use fleet::FleetInbox;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use opencode_codes::client_async::OpencodeClient;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::config::AppConfig;
use crate::server::AdeServer;
use crate::state::{ConnState, Store};
use crate::worktree;

/// Scope key for the repo root (no worktree).
pub(crate) const ROOT_SCOPE: &str = "";

#[derive(Debug, Clone)]
pub struct RuntimeHandle {
    tx: UnboundedSender<Command>,
    slot: Arc<RwLock<Arc<Store>>>,
    inbox: Arc<FleetInbox>,
}

impl RuntimeHandle {
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn snapshot(&self) -> Arc<Store> {
        self.slot.read().map(|g| Arc::clone(&g)).unwrap_or_default()
    }

    /// Register a supervised task (M6 driver: Open-Workspace flow).
    /// Survives server reconnects; drained every poll tick (hook A).
    /// Returns false when a live task already owns the slug (M7d 1:1) —
    /// the task is not registered in that case.
    pub fn register_task(&self, task: Box<dyn fleet::SupervisedTask>) -> bool {
        self.inbox.register_task(task)
    }

    /// Install (or replace) the kanban-lite sweeper.
    pub fn register_sweeper(&self, sweeper: Box<dyn fleet::TaskSweeper>) {
        self.inbox.register_sweeper(sweeper);
    }

    /// The shared fleet registry (driver entry point for task opens).
    pub fn fleet(&self) -> &FleetInbox {
        &self.inbox
    }
}

pub fn spawn(config: AppConfig) -> RuntimeHandle {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let slot: Arc<RwLock<Arc<Store>>> = Arc::new(RwLock::new(Arc::new(Store::default())));
    let inbox = Arc::new(FleetInbox::new());
    let handle = RuntimeHandle {
        tx,
        slot: Arc::clone(&slot),
        inbox: Arc::clone(&inbox),
    };

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime for Dione");
        rt.block_on(outer_loop(config, rx, slot, inbox));
    });

    handle
}

async fn outer_loop(
    config: AppConfig,
    mut rx: UnboundedReceiver<Command>,
    slot: Arc<RwLock<Arc<Store>>>,
    inbox: Arc<FleetInbox>,
) {
    loop {
        match AdeServer::start(&config).await {
            Ok(server) => {
                {
                    let mut st = LoopState::load(&slot);
                    st.store.conn = ConnState::Connected;
                    publish(&slot, &st);
                }
                session::run_session(&config, server, &mut rx, &slot, Arc::clone(&inbox)).await;
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

pub(crate) struct LoopState {
    pub(crate) store: Store,
    pub(crate) base_url: String,
    pub(crate) repo: PathBuf,
    /// Scope ("" = root, else worktree slug) -> directory-scoped client.
    pub(crate) clients: BTreeMap<String, OpencodeClient>,
    pub(crate) pumped: BTreeSet<String>,
}

impl LoopState {
    pub(crate) fn load(slot: &RwLock<Arc<Store>>) -> Self {
        let store = slot.read().map(|g| (**g).clone()).unwrap_or_default();
        Self {
            store,
            base_url: String::new(),
            repo: PathBuf::from("."),
            clients: BTreeMap::new(),
            pumped: BTreeSet::new(),
        }
    }

    pub(crate) fn client_for(&self, sid: &str) -> &OpencodeClient {
        let scope = self.store.scope_of(sid);
        self.clients
            .get(scope)
            .or_else(|| self.clients.get(ROOT_SCOPE))
            .expect("root client always present")
    }
}

pub(crate) fn build_client(
    base_url: &str,
    dir: Option<&std::path::Path>,
) -> anyhow::Result<OpencodeClient> {
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
pub(crate) fn ensure_client(
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

pub(crate) fn spawn_pump(
    client: OpencodeClient,
    slot: Arc<RwLock<Arc<Store>>>,
    scope: String,
    pumped: &mut BTreeSet<String>,
) {
    if !pumped.insert(scope.clone()) {
        return;
    }
    tokio::spawn(async move {
        sse::sse_pump(client, slot, scope).await;
    });
}

pub(crate) fn publish(slot: &RwLock<Arc<Store>>, st: &LoopState) {
    if let Ok(mut guard) = slot.write() {
        *guard = Arc::new(st.store.clone());
    }
}
