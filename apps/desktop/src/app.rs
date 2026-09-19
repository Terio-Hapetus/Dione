//! Dione root view: owns [`DioneApp`] state and the top-level layout.
//! Section renderers live in `views/` (`top_bar`, `sidebar`, `chat`,
//! `composer`, `right_panel`, `diff`, `permission`); shared colors and
//! text helpers live in `views::theme`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base::{Command, DiffNote, RuntimeHandle, Store, UsageSample};
use gpui::*;
use gpui_component::{
    ActiveTheme as _,
    input::{InputEvent, InputState},
};
use workspace::{
    ContainerState, PodmanProvider, WorkspaceProvider as _, container_name_for,
    default_agents_path, load_agents_toml, probe_all, probe_podman,
};

use crate::container_thread::ContainerThread;
use crate::views::terminal::TermState;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum RightTab {
    Context,
    Diff,
    File,
    Costs,
}

pub struct DioneApp {
    pub(crate) rt: RuntimeHandle,
    pub(crate) store: Arc<Store>,
    pub(crate) input: Entity<InputState>,
    pub(crate) right_tab: RightTab,
    pub(crate) model_ix: Option<usize>,
    pub(crate) diff_notes: Vec<DiffNote>,
    /// Single/range note target: `(session, file, start, end)` with
    /// `end = None` for a single line (M8c multi-line).
    pub(crate) annotate_target: Option<(String, String, u32, Option<u32>)>,
    /// Pending range anchor (M8c two-click select): first click waits
    /// for a second click in the same file.
    pub(crate) annotate_anchor: Option<(String, String, u32)>,
    /// Thread reply target (M8c2): composer text appends to this note's
    /// `replies` instead of creating a new note.
    pub(crate) reply_target: Option<DiffNote>,
    /// Cherry-picked hunks (M8a): `(session_id, file, hunk_idx)` selected
    /// in the Diff tab, applied to the main checkout on demand.
    pub(crate) hunk_picks: BTreeSet<(String, String, usize)>,
    /// File open in the viewer tab (M8e, text-first).
    pub(crate) open_file: Option<crate::views::file::OpenFile>,
    /// Podman capability at startup. False → Host-mode banner.
    pub(crate) vm_available: bool,
    /// Workspace slug → container lifecycle state. Filled by the
    /// container thread (ADR-0006); empty until then.
    pub(crate) vm_states: BTreeMap<String, ContainerState>,
    /// Agent registry names from `agents.toml` (M4b), in file order.
    pub(crate) agent_names: Vec<String>,
    /// Name → binary present on `PATH` (green/red tick, Lab 4).
    pub(crate) agent_ok: BTreeMap<String, bool>,
    /// Name → all `env_keys` in the keychain (M9e `◆`/`◇`; absent =
    /// agent declares no keys). Refreshed on the slow loop.
    pub(crate) agent_env: BTreeMap<String, bool>,
    /// Snapshot polls since startup; slow refreshes key off this.
    pub(crate) polls: u64,
    /// Local pty tab state (M6c2). `None` until first opened.
    pub(crate) term: Option<TermState>,
    /// Show terminal instead of chat in the main column.
    pub(crate) show_terminal: bool,
    /// Terminal input row.
    pub(crate) term_input: Entity<InputState>,
    /// Terminal search filter.
    pub(crate) term_query: Entity<InputState>,
    /// New-worktree dialog (UX2): open flag + slug input. Replaces the
    /// old "+ wt steals composer text" behavior.
    pub(crate) show_wt_dialog: bool,
    pub(crate) fleet_input: Entity<InputState>,
    /// Attention filter: only blocked / needs-you / working worktrees.
    pub(crate) fleet_attention_only: bool,
    /// Command palette (UX6): open flag + filter input.
    pub(crate) show_palette: bool,
    pub(crate) palette_input: Entity<InputState>,
    /// Container background thread (ADR-0006). UI sends Ensure/Stop,
    /// drains reports in the snapshot loop — never blocks.
    pub(crate) vm: ContainerThread,
    /// Pending container shell reply. Polled without blocking.
    pub(crate) pending_shell:
        Option<std::sync::mpsc::Receiver<crate::container_thread::ShellReply>>,
    /// True while a guest shell is in flight (placeholder text).
    pub(crate) term_pending: bool,
    /// Usage snapshot for the Costs tab (M9d): drained from the fleet
    /// inbox on the snapshot loop, folded by `views::costs`.
    pub(crate) usage: Vec<UsageSample>,
    /// Pending agent runs waiting for a container to become `Running`
    /// (W1 always-container). Keyed by slug; value is (agent_ref, prompt,
    /// worktree_path). The snapshot loop drains `Running` reports and
    /// spawns via `open_task_with` with a `PodmanProvider`.
    pub(crate) pending_runs: BTreeMap<String, (String, String, std::path::PathBuf)>,
}

/// Snapshot polls per slow refresh: 375 × 160ms ≈ 60s (same cadence as
/// the runtime sweep, cheap enough for a PATH scan + podman probe).
pub(crate) const SLOW_REFRESH_EVERY_POLLS: u64 = 375;

/// Load `(names, probe)` from an `agents.toml` path. `None`/missing →
/// empty (no registry yet, picker shows nothing).
pub(crate) fn load_agent_statuses(path: Option<&Path>) -> (Vec<String>, BTreeMap<String, bool>) {
    let Some(p) = path else {
        return (Vec::new(), BTreeMap::new());
    };
    let reg = load_agents_toml(p);
    let names: Vec<String> = reg.keys().cloned().collect();
    (names, probe_all(&reg))
}

/// Load per-agent keychain presence (M9e BYOK): name → all `env_keys`
/// present in `secrets`. Agents without `env_keys` stay absent (no glyph);
/// values are never read back for display.
pub(crate) fn load_env_status(
    path: Option<&Path>,
    secrets: &dyn workspace::Secrets,
) -> BTreeMap<String, bool> {
    let Some(p) = path else {
        return BTreeMap::new();
    };
    load_agents_toml(p)
        .iter()
        .filter(|(_, e)| !e.env_keys.is_empty())
        .map(|(n, e)| {
            let ok = e.env_keys.iter().all(|k| {
                secrets
                    .get(&workspace::secret_account(n, k))
                    .ok()
                    .flatten()
                    .is_some()
            });
            (n.clone(), ok)
        })
        .collect()
}

/// Keychain backend for presence checks: `None` when no Secret Service
/// CLI is around (slow loop then reports nothing instead of ENOENTs).
fn env_secrets() -> Option<workspace::CliSecrets> {
    workspace::probe_secret_tool().then(workspace::CliSecrets::new)
}

impl DioneApp {
    pub fn new(rt: RuntimeHandle, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Message the agent… (Enter to send)")
                .auto_grow(1, 5)
        });
        cx.subscribe_in(&input, window, |this, _, ev, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                // An anchor alone also captures Enter (as a no-op until
                // the range resolves) so a half-started annotate never
                // fires as a chat prompt by accident.
                if this.annotate_target.is_some()
                    || this.reply_target.is_some()
                    || this.annotate_anchor.is_some()
                {
                    this.submit_annotate(window, cx);
                } else if !this.store.is_busy() {
                    this.send_prompt(window, cx);
                }
            }
        })
        .detach();
        let term_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("shell… (Enter to run)")
                .auto_grow(1, 3)
        });
        let term_query = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("filter…")
                .auto_grow(1, 1)
        });
        cx.subscribe_in(&term_input, window, |this, _, ev, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.send_terminal_input(window, cx);
            }
        })
        .detach();
        let fleet_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("slug… (Enter to create)")
                .auto_grow(1, 1)
        });
        cx.subscribe_in(&fleet_input, window, |this, _, ev, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.create_worktree_from_dialog(window, cx);
            }
        })
        .detach();
        let palette_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("type a command or worktree… (Enter runs first)")
                .auto_grow(1, 1)
        });
        cx.subscribe_in(&palette_input, window, |this, _, ev, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.run_palette_first(window, cx);
            }
        })
        .detach();

        // Snapshot polling — the SSE pump + reconcile live in base's thread.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(160))
                    .await;
                let Ok(snap) = this.read_with(cx, |app, _| app.rt.snapshot()) else {
                    break;
                };
                let _ = this.update(cx, |app, cx| {
                    if !Arc::ptr_eq(&snap, &app.store) {
                        app.store = snap;
                        cx.notify();
                    }
                    // Container reports: feed the Fleet badge without blocking.
                    let mut vm_changed = false;
                    let mut pending_to_spawn: Vec<(String, String, String, std::path::PathBuf)> =
                        Vec::new();
                    let mut pending_to_clear: Vec<String> = Vec::new();
                    for rep in app.vm.drain() {
                        let slug = rep.slug.clone();
                        // Collect pending run for this slug before mutating.
                        if rep.state == ContainerState::Running
                            && let Some((agent, prompt, path)) =
                                app.pending_runs.get(&slug).cloned()
                        {
                            pending_to_spawn.push((slug.clone(), agent, prompt, path));
                        } else if matches!(rep.state, ContainerState::Error(_))
                            && app.pending_runs.contains_key(&slug)
                        {
                            pending_to_clear.push(slug.clone());
                        }
                        app.set_container_state(rep.slug, rep.state);
                        vm_changed = true;
                    }
                    if vm_changed {
                        cx.notify();
                    }
                    // W1 always-container: container became Running → spawn.
                    for (slug, agent, prompt, path) in pending_to_spawn {
                        // Remove before spawn so a spawn error doesn't loop.
                        app.pending_runs.remove(&slug);
                        if let Err(e) = app.spawn_agent_in_container(&slug, &path, &agent, &prompt)
                        {
                            // Surface via store error strip (store is snapshot-
                            // sourced, but push_error on the snapshot Arc is
                            // not persisted — we mutate the Arc's Store via
                            // interior? For W1 we just keep a local error via
                            // the store's errors are not directly writable.
                            // Use tracing + keep pending cleared so retry
                            // requires another click.
                            tracing::warn!("run {slug} spawn failed: {e:#}");
                        }
                        cx.notify();
                    }
                    for slug in pending_to_clear {
                        app.pending_runs.remove(&slug);
                        cx.notify();
                    }
                    // Usage snapshot for the Costs tab (M9d): cheap
                    // Vec compare, notify only on change.
                    let usage = app.rt.fleet().collect_usage();
                    if usage != app.usage {
                        app.usage = usage;
                        cx.notify();
                    }
                    // Guest shell arrival (W3).
                    if app.poll_pending_shell() {
                        cx.notify();
                    }
                    // Slow refresh: pick up agents.toml edits, newly
                    // installed binaries, and podman install without restart.
                    app.polls += 1;
                    if app.polls.is_multiple_of(SLOW_REFRESH_EVERY_POLLS) {
                        app.refresh_agents();
                        app.vm_available = probe_podman();
                        cx.notify();
                    }
                    // Terminal pump: non-blocking scrollback drain.
                    if app.show_terminal && app.pump_terminal() {
                        cx.notify();
                    }
                });
            }
        })
        .detach();

        let store = rt.snapshot();
        let (agent_names, agent_ok) = load_agent_statuses(default_agents_path().as_deref());
        let agent_env = env_secrets()
            .map(|s| load_env_status(default_agents_path().as_deref(), &s))
            .unwrap_or_default();
        let vm = ContainerThread::spawn();
        Self {
            rt,
            store,
            input,
            vm,
            term_input,
            term_query,
            right_tab: RightTab::Context,
            model_ix: None,
            diff_notes: Vec::new(),
            annotate_target: None,
            annotate_anchor: None,
            reply_target: None,
            hunk_picks: BTreeSet::new(),
            open_file: None,
            vm_available: probe_podman(),
            vm_states: BTreeMap::new(),
            agent_names,
            agent_ok,
            agent_env,
            polls: 0,
            term: None,
            show_terminal: false,
            show_wt_dialog: false,
            fleet_input,
            fleet_attention_only: false,
            show_palette: false,
            palette_input,
            pending_shell: None,
            term_pending: false,
            usage: Vec::new(),
            pending_runs: BTreeMap::new(),
        }
    }

    /// Create a worktree from the Fleet dialog (UX2). Empty input →
    /// auto-name `task-N`; dialog always closes on submit.
    pub(crate) fn create_worktree_from_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.show_wt_dialog {
            return;
        }
        let typed = self.fleet_input.read(cx).value().to_string();
        let slug = if typed.trim().is_empty() {
            format!("task-{}", self.store.worktrees.len() + 1)
        } else {
            typed.trim().to_string()
        };
        self.rt.send(Command::CreateWorktree { slug });
        self.fleet_input
            .update(cx, |st, cx| st.set_value("", window, cx));
        self.show_wt_dialog = false;
        cx.notify();
    }

    /// Re-probe agent binaries (slow-refresh loop + registry edits).
    pub(crate) fn refresh_agents(&mut self) {
        let (names, ok) = load_agent_statuses(default_agents_path().as_deref());
        self.agent_names = names;
        self.agent_ok = ok;
        self.agent_env = env_secrets()
            .map(|s| load_env_status(default_agents_path().as_deref(), &s))
            .unwrap_or_default();
    }

    /// Record a workspace container state for the Fleet badge.
    /// Fed by the container thread via the snapshot loop (ADR-0006/0007).
    pub(crate) fn set_container_state(&mut self, slug: String, state: ContainerState) {
        self.vm_states.insert(slug, state);
    }

    /// Pick the agent for ▶ Run: first present CLI in registry order.
    /// `None` = no registry or nothing on PATH — caller surfaces an error.
    pub(crate) fn pick_run_agent(&self) -> Option<String> {
        for name in &self.agent_names {
            if self.agent_ok.get(name).copied().unwrap_or(false) {
                return Some(name.clone());
            }
        }
        None
    }

    /// Spawn a terminal agent in the worktree's container (W1/W2
    /// always-container). Any `agent_ref` (registry key like `claude` or
    /// `codex`) is treated as a terminal CLI over the container pty — the
    /// exact binary is resolved at exec time inside the container, not
    /// here. Secrets are attached when a keychain is present (M9e).
    pub(crate) fn spawn_agent_in_container(
        &self,
        slug: &str,
        path: &Path,
        agent_ref: &str,
        prompt: &str,
    ) -> anyhow::Result<()> {
        if self.rt.fleet().has_slug(slug) {
            anyhow::bail!("task already open for slug {slug:?}");
        }
        if !self.rt.fleet().has_sweeper() {
            self.rt
                .fleet()
                .register_sweeper(Box::new(agent::sweeper::FleetSweeper::new()));
        }
        let root = crate::container_thread::workspace_root(path);
        let key = crate::container_thread::worktree_key(path);
        let name = container_name_for(&key);
        let mut provider = PodmanProvider::new(&name, &root);
        let shell = provider.shell(path)?;
        let mut adapter = agent::terminal::TerminalAdapter::new();
        let task = workspace::Task::new(slug, agent_ref);
        let id = task.id;
        adapter.attach(id, shell);
        // Queue the first prompt (same as driver.rs).
        {
            let backend: &mut dyn agent::agent::AgentBackend = &mut adapter;
            backend.spawn(id, prompt)?;
        }
        let ws: Box<dyn workspace::WorkspaceProvider> = Box::new(provider);
        let mut sup = agent::supervisor::Supervisor::new(task, Box::new(adapter), ws);
        if let Some(secrets) = env_secrets()
            && let Some(p) = default_agents_path()
        {
            let reg = load_agents_toml(&p);
            if reg.get(agent_ref).is_some_and(|e| !e.env_keys.is_empty()) {
                sup = sup.with_secrets(reg, Box::new(secrets));
            }
        }
        if !self.rt.fleet().register_task(Box::new(sup)) {
            anyhow::bail!("task already open (lost registration race)");
        }
        Ok(())
    }

    /// ▶ Run clicked for a worktree row (W1). Always-container: ensure
    /// to `Running` then spawn, or spawn immediately if already `Running`.
    /// Empty prompt or no agent → no-ops (pending not inserted) so the
    /// user can fix the input without a stuck pending.
    pub(crate) fn run_worktree_agent(
        &mut self,
        slug: String,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        if !self.vm_available {
            tracing::warn!("run {slug}: podman unavailable — Host mode");
            return;
        }
        let prompt = self.input.read(cx).value().to_string();
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            tracing::warn!("run {slug}: empty prompt — fill the composer first");
            return;
        }
        let agent_ref = match self.pick_run_agent() {
            Some(a) => a,
            None => {
                tracing::warn!("run {slug}: no agent binary found on PATH");
                return;
            }
        };
        if self.rt.fleet().has_slug(&slug) {
            tracing::warn!("run {slug}: task already open");
            return;
        }
        // The composer text becomes the agent prompt: drop any pending
        // annotate/reply targeting so note text can never mis-fire as a
        // run prompt.
        self.annotate_target = None;
        self.annotate_anchor = None;
        self.reply_target = None;
        let state = self.vm_states.get(&slug).cloned();
        match state {
            Some(ContainerState::Running) => {
                if let Err(e) = self.spawn_agent_in_container(&slug, &path, &agent_ref, &prompt) {
                    tracing::warn!("run {slug} spawn failed: {e:#}");
                }
            }
            Some(ContainerState::Paused) => {
                self.pending_runs
                    .insert(slug.clone(), (agent_ref, prompt, path.clone()));
                self.vm.unpause(slug, path);
            }
            _ => {
                self.pending_runs
                    .insert(slug.clone(), (agent_ref, prompt, path.clone()));
                self.vm.ensure(slug, path);
            }
        }
        cx.notify();
    }

    /// ADR-0007: selecting a worktree wakes its container and pauses
    /// idle ones (lightweight). Only worktrees with no live task and no
    /// busy session sleep — active ones stay `Running` even off-screen.
    pub(crate) fn select_worktree(&mut self, slug: String) {
        let selected_path = self.store.worktrees.get(&slug).map(|r| r.path.clone());
        self.rt.send(Command::SelectWorktree { slug: slug.clone() });
        if let Some(path) = selected_path {
            let cur = self.vm_states.get(&slug).cloned();
            if !matches!(
                cur,
                Some(ContainerState::Running) | Some(ContainerState::Paused)
            ) {
                self.vm.ensure(slug.clone(), path.clone());
            } else if cur == Some(ContainerState::Paused) {
                self.vm.unpause(slug.clone(), path.clone());
            }
            // Pause idle worktrees (not the selected one, not live).
            for (other, rec) in self.store.worktrees.clone() {
                if other == slug {
                    continue;
                }
                if self.vm_states.get(&other) != Some(&ContainerState::Running) {
                    continue;
                }
                if self.store.is_blocked_slug(&other) {
                    continue;
                }
                // Live terminal-agent task (session-less): never pause
                // mid-run. `FleetInbox::has_slug` is the same 1:1 gate
                // the driver uses before opening a task.
                if self.rt.fleet().has_slug(&other) {
                    continue;
                }
                let busy = self
                    .store
                    .sessions_in_scope(&other)
                    .iter()
                    .any(|sid| self.store.is_busy_scope(sid));
                if busy {
                    continue;
                }
                // Live task check would need fleet access; rely on session
                // busyness + blocked flag for now (task tasks are session-
                // backed). The thread's pause is idempotent, so no inspect
                // cost — just send.
                self.vm.pause(other.clone(), rec.path.clone());
            }
        }
    }

    pub(crate) fn send_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.store.active_session.is_none() {
            return;
        }
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() || self.store.is_busy() {
            return;
        }
        self.rt.send(Command::Prompt { text });
        self.input.update(cx, |st, cx| st.set_value("", window, cx));
    }

    pub(crate) fn send_fan_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        // Same busy gate as `send_prompt`: fan-out while the active
        // session is busy would interleave prompts mid-stream.
        if text.trim().is_empty() || self.store.is_busy() {
            return;
        }
        self.rt.send(Command::FanOut { text });
        self.input.update(cx, |st, cx| st.set_value("", window, cx));
    }

    /// Submit the composer text as a review note on the targeted diff
    /// line or range — or, when replying, as a thread reply under the
    /// targeted note (M8c2).
    pub(crate) fn submit_annotate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        if let Some(target) = self.reply_target.clone() {
            // Target deleted meanwhile → keep the text so the user can
            // re-target instead of losing the reply silently.
            if base::append_reply(&mut self.diff_notes, &target, text.trim().to_string()) {
                self.reply_target = None;
                self.input.update(cx, |st, cx| st.set_value("", window, cx));
            }
        } else {
            let Some((sid, file, line, end)) = self.annotate_target.clone() else {
                return;
            };
            self.diff_notes.push(DiffNote {
                session_id: sid,
                file,
                line,
                end_line: end,
                text: text.trim().to_string(),
                replies: Vec::new(),
            });
            self.annotate_target = None;
            self.annotate_anchor = None;
            self.input.update(cx, |st, cx| st.set_value("", window, cx));
        }
        cx.notify();
    }

    pub(crate) fn flat_models(&self) -> Vec<(String, String)> {
        self.store
            .providers
            .iter()
            .flat_map(|p| {
                p.models
                    .iter()
                    .map(move |(id, _)| (p.provider_id.clone(), id.clone()))
            })
            .collect()
    }

    pub(crate) fn pick_model(&mut self, ix: usize) {
        let models = self.flat_models();
        if let Some((provider_id, model_id)) = models.get(ix) {
            self.model_ix = Some(ix);
            self.rt.send(Command::SetModel {
                provider_id: provider_id.clone(),
                model_id: model_id.clone(),
            });
        }
    }

    pub(crate) fn model_label(&self, models: &[(String, String)]) -> String {
        match self.model_ix.and_then(|ix| models.get(ix)) {
            Some((p, m)) => format!("{p}/{m}"),
            None => "model: default".into(),
        }
    }
}

impl Render for DioneApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bg = cx.theme().background;
        let fg = cx.theme().foreground;
        // Permission queue: overlay the first, badge the depth (UX7).
        let queue = self.store.pending_permissions.len();
        let pending = self.store.pending_permissions.values().next().cloned();

        div()
            .id("dione-root")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .text_size(px(13.))
            .child(self.render_titlebar(window, cx))
            .child(self.render_top_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_activity_bar(cx))
                    .child(self.render_sidebar(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(self.render_worktree_tabs(cx))
                            .child(self.render_vm_banner())
                            .child(self.render_error_strip(cx))
                            .child(if self.show_terminal {
                                self.render_terminal(cx)
                            } else {
                                self.render_chat(window, cx)
                            })
                            .child(self.render_composer(cx)),
                    )
                    .child(self.render_right_panel(cx)),
            )
            .child(self.render_status_bar(cx))
            .children(pending.map(|p| self.render_permission_overlay(p, queue, cx)))
            .children(self.show_palette.then(|| self.render_palette(cx)))
    }
}
