//! ADE root view: owns [`AdeApp`] state and the top-level layout.
//! Section renderers live in `views/` (`top_bar`, `sidebar`, `chat`,
//! `composer`, `right_panel`, `diff`, `permission`); shared colors and
//! text helpers live in `views::theme`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use ade_core::{Command, DiffNote, RuntimeHandle, Store};
use ade_vm::{VmState, probe_kvm};
use gpui::*;
use gpui_component::{
    ActiveTheme as _,
    input::{InputEvent, InputState},
};

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum RightTab {
    Context,
    Inspector,
    Diff,
}

pub struct AdeApp {
    pub(crate) rt: RuntimeHandle,
    pub(crate) store: Arc<Store>,
    pub(crate) input: Entity<InputState>,
    pub(crate) right_tab: RightTab,
    pub(crate) model_ix: Option<usize>,
    pub(crate) diff_notes: Vec<DiffNote>,
    pub(crate) annotate_target: Option<(String, String, u32)>,
    /// KVM capability at startup. False → Host-mode banner (Lab 6).
    pub(crate) vm_available: bool,
    /// Workspace slug → VM lifecycle state. Filled by the VmManager
    /// wiring (post-M5 Open-Workspace flow); empty until then.
    pub(crate) vm_states: BTreeMap<String, VmState>,
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
                if this.annotate_target.is_some() {
                    this.submit_annotate(window, cx);
                } else if !this.store.is_busy() {
                    this.send_prompt(window, cx);
                }
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
                });
            }
        })
        .detach();

        let store = rt.snapshot();
        Self {
            rt,
            store,
            input,
            right_tab: RightTab::Context,
            model_ix: None,
            diff_notes: Vec::new(),
            annotate_target: None,
            vm_available: probe_kvm(),
            vm_states: BTreeMap::new(),
        }
    }

    /// Record a workspace VM state for the Fleet badge.
    /// Called by the VmManager thread (post-M5 Open-Workspace flow).
    #[allow(dead_code)]
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

    /// Submit the composer text as a review note on the targeted diff line.
    pub(crate) fn submit_annotate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((sid, file, line)) = self.annotate_target.clone() else {
            return;
        };
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        self.diff_notes.push(DiffNote {
            session_id: sid,
            file,
            line,
            text: text.trim().to_string(),
        });
        self.annotate_target = None;
        self.input.update(cx, |st, cx| st.set_value("", window, cx));
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
                    .child(self.render_sidebar(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(self.render_vm_banner())
                            .child(self.render_error_strip())
                            .child(self.render_chat(window, cx))
                            .child(self.render_composer(cx)),
                    )
                    .child(self.render_right_panel(cx)),
            )
            .children(pending.map(|p| self.render_permission_overlay(p, cx)))
    }
}
