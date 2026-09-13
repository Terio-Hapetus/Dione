//! ADE root view: owns [`AdeApp`] state and the top-level layout.
//! Section renderers live in `views/` (`top_bar`, `sidebar`, `chat`,
//! `composer`, `right_panel`, `diff`, `permission`); shared colors and
//! text helpers live in `views::theme`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ade_core::{Command, DiffNote, RuntimeHandle, Store};
use ade_vm::{SshInfo, VmState, probe_kvm};
use ade_workspace::{default_agents_path, load_agents_toml, probe_all};
use gpui::*;
use gpui_component::{
    ActiveTheme as _,
    input::{InputEvent, InputState},
};

use crate::views::terminal::TermState;
use crate::vm_thread::VmThread;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum RightTab {
    Context,
    Inspector,
    Diff,
    File,
}

pub struct AdeApp {
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
    /// KVM capability at startup. False → Host-mode banner (Lab 6).
    pub(crate) vm_available: bool,
    /// Workspace slug → VM lifecycle state. Filled by the VmManager
    /// wiring (post-M5 Open-Workspace flow); empty until then.
    pub(crate) vm_states: BTreeMap<String, VmState>,
    /// Agent registry names from `agents.toml` (M4b), in file order.
    pub(crate) agent_names: Vec<String>,
    /// Name → binary present on `PATH` (green/red tick, Lab 4).
    pub(crate) agent_ok: BTreeMap<String, bool>,
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
    /// VM manager background thread (W2). UI sends Ensure/Stop, drains
    /// reports in the snapshot loop — never blocks.
    pub(crate) vm: VmThread,
    /// SSH endpoints per worktree slug (for the badge + manual access).
    pub(crate) vm_ssh: BTreeMap<String, SshInfo>,
    /// Pending guest shell reply (W3). Polled without blocking.
    pub(crate) pending_shell: Option<std::sync::mpsc::Receiver<crate::vm_thread::ShellReply>>,
    /// True while a guest shell is in flight (placeholder text).
    pub(crate) term_pending: bool,
}

/// Snapshot polls per slow refresh: 375 × 160ms ≈ 60s (same cadence as
/// the runtime sweep, cheap enough for a PATH scan + KVM probe).
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

impl AdeApp {
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

        // Snapshot polling — the SSE pump + reconcile live in ade-core's thread.
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
                    // VM reports: feed the Fleet badge without blocking.
                    let mut vm_changed = false;
                    for rep in app.vm.drain() {
                        match rep.ssh {
                            Some(ssh) => {
                                app.vm_ssh.insert(rep.slug.clone(), ssh);
                            }
                            None => {
                                app.vm_ssh.remove(&rep.slug);
                            }
                        }
                        app.set_vm_state(rep.slug, rep.state);
                        vm_changed = true;
                    }
                    if vm_changed {
                        cx.notify();
                    }
                    // Guest shell arrival (W3).
                    if app.poll_pending_shell() {
                        cx.notify();
                    }
                    // Slow refresh: pick up agents.toml edits, newly
                    // installed binaries, and KVM hotplug without restart.
                    app.polls += 1;
                    if app.polls.is_multiple_of(SLOW_REFRESH_EVERY_POLLS) {
                        app.refresh_agents();
                        app.vm_available = probe_kvm();
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
        let vm = VmThread::spawn();
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
            vm_available: probe_kvm(),
            vm_states: BTreeMap::new(),
            vm_ssh: BTreeMap::new(),
            agent_names,
            agent_ok,
            polls: 0,
            term: None,
            show_terminal: false,
            show_wt_dialog: false,
            fleet_input,
            fleet_attention_only: false,
            pending_shell: None,
            term_pending: false,
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
    }

    /// Record a workspace VM state for the Fleet badge.
    /// Fed by the VmManager thread via the snapshot loop (W2).
    pub(crate) fn set_vm_state(&mut self, slug: String, state: VmState) {
        self.vm_states.insert(slug, state);
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
        if text.trim().is_empty() {
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
            if ade_core::append_reply(&mut self.diff_notes, &target, text.trim().to_string()) {
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

impl Render for AdeApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bg = cx.theme().background;
        let fg = cx.theme().foreground;
        let pending = self.store.pending_permissions.values().next().cloned();

        div()
            .id("ade-root")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .text_size(px(13.))
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
            .children(pending.map(|p| self.render_permission_overlay(p, cx)))
    }
}
