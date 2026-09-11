use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;

use super::LoopState;
use super::commands::Command;
use super::fleet::{apply_sweep, drain_fleet, sweep_due};
use super::handlers::handle_command;
use super::reconcile::{fetch_providers, fetch_todos, reconcile_all_sessions, reconcile_messages};
use super::{ROOT_SCOPE, publish, spawn_pump};
use crate::config::AppConfig;
use crate::server::AdeServer;
use crate::worktree::{self, WorktreeRecord};

pub(crate) async fn run_session(
    config: &AppConfig,
    server: AdeServer,
    rx: &mut UnboundedReceiver<Command>,
    slot: &Arc<RwLock<Arc<crate::state::Store>>>,
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
                let Some(cmd) = cmd else { return; }; // UI gone: end session loop.
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

pub(crate) async fn poll_once(st: &mut LoopState, tick: u64) {
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
    // M4 fleet hook A: after reconcile, before publish. Empty fleet and
    // no sweeper keep this a no-op until slices 2+ register tasks.
    drain_fleet(&mut st.fleet, &mut st.store);
    if sweep_due(tick)
        && let Some(sw) = &st.sweeper
    {
        apply_sweep(&mut st.store, &sw.sweep());
    }
}

pub(crate) async fn bootstrap(st: &mut LoopState) {
    discover_worktrees(st).await;
    reconcile_all_sessions(st).await;
    if let Some(root) = st.clients.get(ROOT_SCOPE).cloned() {
        fetch_providers(st, &root).await;
    }
}

/// Adopt on-disk managed worktrees (e.g. from a previous run) into the store.
pub(crate) async fn discover_worktrees(st: &mut LoopState) {
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
