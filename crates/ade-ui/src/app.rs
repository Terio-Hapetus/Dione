//! M1 root view: sessions sidebar, timeline, composer, right panel.

use std::sync::Arc;
use std::time::Duration;

use ade_core::state::MessageEntry;
use ade_core::{Command, ConnState, PermissionResponse, RuntimeHandle, Store, WorktreeStatus};
use ade_core::{DiffNote, Role, UnifiedMessage, parse_patch_lines};
use gpui::*;
use gpui_component::{
    ActiveTheme, Disableable as _, Icon, IconName, Sizable as _,
    button::Button,
    input::{Input, InputEvent, InputState},
    label::Label,
    text::TextView,
};
use opencode_codes::protocol_generated::types::{Message, Part, SessionStatus, ToolState};

fn ok_color() -> Rgba {
    rgba(0x3fd17cff)
}
fn bad_color() -> Rgba {
    rgba(0xe5484dff)
}
fn warn_color() -> Rgba {
    rgba(0xf5c542ff)
}
fn muted_color() -> Rgba {
    rgba(0x8b8b93ff)
}
fn soft_border() -> Rgba {
    rgba(0x33363fff)
}

#[derive(Clone, Copy, PartialEq)]
enum RightTab {
    Context,
    Inspector,
    Diff,
}

pub struct AdeApp {
    rt: RuntimeHandle,
    store: Arc<Store>,
    input: Entity<InputState>,
    right_tab: RightTab,
    selected_part: Option<String>,
    model_ix: Option<usize>,
    diff_notes: Vec<DiffNote>,
    annotate_target: Option<(String, String, u32)>,
    /// M3d: v2 chat (reads `transcripts`) side by side with the legacy timeline.
    chat_v2: bool,
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
            selected_part: None,
            model_ix: None,
            diff_notes: Vec::new(),
            annotate_target: None,
            chat_v2: false,
        }
    }

    fn send_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn send_fan_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        self.rt.send(Command::FanOut { text });
        self.input.update(cx, |st, cx| st.set_value("", window, cx));
    }

    /// Submit the composer text as a review note on the targeted diff line.
    fn submit_annotate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn flat_models(&self) -> Vec<(String, String)> {
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

    fn pick_model(&mut self, ix: usize) {
        let models = self.flat_models();
        if let Some((provider_id, model_id)) = models.get(ix) {
            self.model_ix = Some(ix);
            self.rt.send(Command::SetModel {
                provider_id: provider_id.clone(),
                model_id: model_id.clone(),
            });
        }
    }

    fn model_label(&self, models: &[(String, String)]) -> String {
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
                            .child(self.render_error_strip())
                            .child(if self.chat_v2 {
                                self.render_chat(window, cx)
                            } else {
                                self.render_timeline(window, cx).into_any_element()
                            })
                            .child(self.render_composer(cx)),
                    )
                    .child(self.render_right_panel(cx)),
            )
            .children(pending.map(|p| self.render_permission_overlay(p, cx)))
    }
}

// ---------------------------------------------------------------- top bar

impl AdeApp {
    fn render_top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (dot, status_text) = match &self.store.conn {
            ConnState::Connected => (ok_color(), "connected"),
            ConnState::Connecting => (warn_color(), "connecting…"),
            ConnState::Disconnected => (bad_color(), "disconnected"),
        };
        let t = &self.store.totals;
        let border = cx.theme().border;

        let models = self.flat_models();
        let current = self.model_label(&models);
        let n = models.len();
        let next = cx.listener(move |this, _: &ClickEvent, _, _| {
            if n > 0 {
                let ix = this.model_ix.map(|i| (i + 1) % n).unwrap_or(0);
                this.pick_model(ix);
            }
        });
        let prev = cx.listener(move |this, _: &ClickEvent, _, _| {
            if n > 0 {
                this.pick_model(this.model_ix.map(|i| (i + n - 1) % n).unwrap_or(n - 1));
            }
        });

        div()
            .h(px(38.))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(border)
            .child(Icon::new(IconName::CircleCheck).small().text_color(dot))
            .child(Label::new(status_text).text_color(muted_color()))
            .child(div().w(px(1.)).h(px(16.)).bg(border))
            .child(
                Button::new("model-prev")
                    .label("<")
                    .xsmall()
                    .compact()
                    .on_click(prev),
            )
            .child(Label::new(current))
            .child(
                Button::new("model-next")
                    .label(">")
                    .xsmall()
                    .compact()
                    .on_click(next),
            )
            .child(div().flex_1())
            .child(
                Button::new("chat-toggle")
                    .label(if self.chat_v2 { "chat v2" } else { "timeline" })
                    .xsmall()
                    .compact()
                    .on_click(cx.listener(|this, _: &ClickEvent, _, _| {
                        this.chat_v2 = !this.chat_v2;
                    })),
            )
            .child(
                Label::new(format!(
                    "ctx≈{:.1}k tok · ${:.4}",
                    t.total_context() / 1000.0,
                    t.cost
                ))
                .text_color(muted_color()),
            )
    }
}

// --------------------------------------------------------------- sidebar

impl AdeApp {
    fn session_dot(&self, id: &str) -> Option<Rgba> {
        self.store.statuses.get(id).and_then(|st| match st {
            SessionStatus::Busy => Some(ok_color()),
            SessionStatus::Retry { .. } => Some(warn_color()),
            SessionStatus::Idle => None,
        })
    }

    fn worktree_dot(&self, slug: &str) -> Option<Rgba> {
        match self.store.worktree_status(slug) {
            WorktreeStatus::Working => Some(ok_color()),
            WorktreeStatus::NeedsYou => Some(warn_color()),
            WorktreeStatus::Creating | WorktreeStatus::Done => None,
        }
    }

    fn session_row(
        &self,
        id: &str,
        title: String,
        indent: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.store.active_session.as_deref() == Some(id);
        let dot = self.session_dot(id);
        let row_id = SharedString::from(format!("ses-row-{id}"));
        let select_id = id.to_string();
        let bg: Hsla = if active {
            rgba(0x2a3044ff).into()
        } else {
            Hsla::transparent_black()
        };
        let select = cx.listener(move |this, _: &ClickEvent, _, _| {
            this.rt.send(Command::SelectSession {
                id: select_id.clone(),
            });
        });
        div()
            .id(row_id)
            .h(px(30.))
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .px_2()
            .children(indent.then(|| div().w(px(12.)).flex_none()))
            .bg(bg)
            .cursor_pointer()
            .on_click(select)
            .child(Label::new(title).text_size(px(12.)))
            .children(dot.map(|c| Icon::new(IconName::CircleCheck).xsmall().text_color(c)))
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let new_session = cx.listener(|this, _: &ClickEvent, _, _| {
            this.rt.send(Command::CreateSession {
                title: String::new(),
            });
        });
        // "+ wt" uses the composer text as slug (handy: type task, click +wt),
        // else auto-names task-N.
        let new_worktree = cx.listener(|this, _: &ClickEvent, window, cx| {
            let typed = this.input.read(cx).value().to_string();
            let slug = if typed.trim().is_empty() {
                format!("task-{}", this.store.worktrees.len() + 1)
            } else {
                typed
            };
            this.rt.send(Command::CreateWorktree { slug });
            this.input.update(cx, |st, cx| st.set_value("", window, cx));
        });

        let mut list = div()
            .id("sessions")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();

        // Worktree groups, each with its sessions.
        let mut slugs: Vec<_> = self.store.worktrees.keys().cloned().collect();
        slugs.sort();
        for slug in slugs {
            let active = self.store.active_worktree.as_deref() == Some(slug.as_str());
            let dot = self.worktree_dot(&slug);
            let select_slug = slug.clone();
            let select = cx.listener(move |this, _: &ClickEvent, _, _| {
                this.rt.send(Command::SelectWorktree {
                    slug: select_slug.clone(),
                });
            });
            let remove_slug = slug.clone();
            let remove = cx.listener(move |this, _: &ClickEvent, _, _| {
                this.rt.send(Command::RemoveWorktree {
                    slug: remove_slug.clone(),
                });
            });
            let bg: Hsla = if active {
                rgba(0x2a3044ff).into()
            } else {
                Hsla::transparent_black()
            };
            list =
                list.child(
                    div()
                        .id(SharedString::from(format!("wt-row-{slug}")))
                        .h(px(30.))
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_2()
                        .bg(bg)
                        .cursor_pointer()
                        .on_click(select)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .children(dot.map(|c| {
                                    Icon::new(IconName::CircleCheck).xsmall().text_color(c)
                                }))
                                .child(Label::new(format!("⑂ {slug}")).text_size(px(12.))),
                        )
                        .child(
                            Button::new(SharedString::from(format!("wt-del-{slug}")))
                                .label("×")
                                .xsmall()
                                .compact()
                                .on_click(remove),
                        ),
                );
            for sid in self.store.sessions_in_scope(&slug) {
                let title = self
                    .store
                    .sessions
                    .get(&sid)
                    .map(|s| truncate(&s.title, 20))
                    .unwrap_or_else(|| "(gone)".into());
                list = list.child(self.session_row(&sid, title, true, cx));
            }
        }

        // Root sessions (no worktree).
        for sid in self.store.sessions_in_scope("") {
            let title = self
                .store
                .sessions
                .get(&sid)
                .map(|s| truncate(&s.title, 24))
                .unwrap_or_else(|| "(gone)".into());
            list = list.child(self.session_row(&sid, title, false, cx));
        }

        div()
            .w(px(220.))
            .flex_none()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .child(Label::new("Fleet"))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                Button::new("wt-new")
                                    .label("+ wt")
                                    .xsmall()
                                    .compact()
                                    .on_click(new_worktree),
                            )
                            .child(
                                Button::new("session-new")
                                    .label("+ new")
                                    .xsmall()
                                    .compact()
                                    .on_click(new_session),
                            ),
                    ),
            )
            .child(list)
    }

    fn render_error_strip(&self) -> impl IntoElement {
        let last = self.store.errors.back().cloned();
        div().children(last.map(|e| {
            div()
                .flex()
                .gap_2()
                .px_3()
                .py_1()
                .border_b_1()
                .border_color(bad_color())
                .child(Label::new("⚠").text_color(bad_color()))
                .child(Label::new(truncate(&e, 200)).text_color(bad_color()))
        }))
    }
}

// --------------------------------------------------------------- timeline

impl AdeApp {
    fn render_timeline(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(entries) = self.store.active_messages() else {
            return v_center("Create a session in the sidebar to begin.");
        };
        if entries.is_empty() {
            return v_center("No messages yet — say something below.");
        }

        let entries: Vec<MessageEntry> = entries.clone();
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut text_part_ix: usize = 0;

        for entry in entries {
            match &entry.info {
                Message::User(_) => {
                    let text = entry
                        .parts
                        .iter()
                        .filter_map(|p| match p {
                            Part::Text(t) => Some(t.text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if text.is_empty() {
                        continue;
                    }
                    rows.push(
                        div()
                            .flex()
                            .justify_end()
                            .pb_3()
                            .pt_1()
                            .child(
                                div()
                                    .max_w(px(640.))
                                    .rounded_md()
                                    .px_3()
                                    .py_2()
                                    .bg(rgb(0x242838))
                                    .child(text),
                            )
                            .into_any_element(),
                    );
                }
                Message::Assistant(a) => {
                    rows.push(
                        div()
                            .flex()
                            .gap_2()
                            .items_center()
                            .pt_2()
                            .child(
                                Label::new(format!("AGENT · {}", a.model_id))
                                    .text_size(px(11.))
                                    .text_color(muted_color()),
                            )
                            .children((a.cost > 0.).then(|| {
                                Label::new(format!("${:.4}", a.cost))
                                    .text_size(px(11.))
                                    .text_color(muted_color())
                            }))
                            .into_any_element(),
                    );
                    for part in &entry.parts {
                        rows.push(self.part_row(part, text_part_ix, window, cx));
                        if matches!(part, Part::Text(_)) {
                            text_part_ix += 1;
                        }
                    }
                    rows.push(div().h(px(6.)).into_any_element());
                }
            }
        }

        match self
            .store
            .active_session
            .as_ref()
            .and_then(|id| self.store.statuses.get(id))
        {
            Some(SessionStatus::Busy) | Some(SessionStatus::Retry { .. }) => {
                rows.push(
                    Label::new("▌ agent working…")
                        .text_color(ok_color())
                        .into_any_element(),
                );
            }
            _ => {}
        }

        div()
            .id("timeline")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_3()
            .py_2()
            .children(rows)
            .into_any_element()
    }

    /// M3d: agent-agnostic chat. Reads only `transcripts`/`costs` —
    /// no opencode wire types. Legacy timeline stays until parity.
    fn render_chat(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(sid) = self.store.active_session.clone() else {
            return v_center("Create a session in the sidebar to begin.");
        };
        let msgs: Vec<UnifiedMessage> = self.store.transcript_for_session(&sid).to_vec();
        if msgs.is_empty() {
            return v_center("No unified messages yet — say something below.");
        }
        let mut rows: Vec<AnyElement> = Vec::new();
        if let Some(cost) = self
            .store
            .session_task
            .get(&sid)
            .and_then(|t| self.store.costs.get(t))
        {
            rows.push(
                Label::new(format!("{} messages · ${:.4}", msgs.len(), cost.cost))
                    .text_size(px(11.))
                    .text_color(muted_color())
                    .into_any_element(),
            );
        }
        for m in &msgs {
            if m.text.trim().is_empty() {
                continue;
            }
            rows.push(self.unified_row(m, window, cx));
        }
        if self.store.is_busy() {
            rows.push(
                Label::new("▌ agent working…")
                    .text_color(ok_color())
                    .into_any_element(),
            );
        }
        div()
            .id("chat-v2")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_3()
            .py_2()
            .children(rows)
            .into_any_element()
    }

    fn unified_row(
        &self,
        m: &UnifiedMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match m.role {
            Role::User => div()
                .flex()
                .justify_end()
                .pb_3()
                .pt_1()
                .child(
                    div()
                        .max_w(px(640.))
                        .rounded_md()
                        .px_3()
                        .py_2()
                        .bg(rgb(0x242838))
                        .child(m.text.clone()),
                )
                .into_any_element(),
            Role::Agent => div()
                .flex()
                .justify_start()
                .pb_2()
                .child(div().max_w(px(640.)).child(TextView::markdown(
                    SharedString::from(format!("chat-{}", m.id)),
                    m.text.clone(),
                    window,
                    cx,
                )))
                .into_any_element(),
            Role::Tool => {
                let title = m
                    .tool
                    .as_ref()
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| "tool".to_string());
                div()
                    .my_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(soft_border())
                    .px_2()
                    .py_1()
                    .child(
                        Label::new(format!("🔧 {title}"))
                            .text_size(px(11.))
                            .text_color(ok_color()),
                    )
                    .child(
                        Label::new(truncate(&m.text, 500))
                            .text_size(px(11.))
                            .text_color(muted_color()),
                    )
                    .into_any_element()
            }
        }
    }

    fn part_row(
        &self,
        part: &Part,
        text_part_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let json = serde_json::to_string_pretty(part).unwrap_or_default();
        let inspect = {
            let json = json.clone();
            cx.listener(move |app, _: &ClickEvent, _, _| {
                app.selected_part = Some(json.clone());
                app.right_tab = RightTab::Inspector;
            })
        };

        let content: AnyElement = match part {
            Part::StepStart(_) | Part::StepFinish(_) => {
                return div().into_any_element();
            }
            Part::Text(t) => {
                if t.synthetic.unwrap_or(false) || t.text.trim().is_empty() {
                    return div().into_any_element();
                }
                TextView::markdown(
                    SharedString::from(format!("md-{text_part_ix}")),
                    t.text.clone(),
                    window,
                    cx,
                )
                .into_any_element()
            }
            Part::Reasoning(r) => {
                if r.text.trim().is_empty() {
                    return div().into_any_element();
                }
                div()
                    .border_l_2()
                    .border_color(muted_color())
                    .pl_2()
                    .my_1()
                    .child(
                        Label::new(format!("thinking… {}", truncate(&r.text, 400)))
                            .text_size(px(11.))
                            .text_color(muted_color()),
                    )
                    .into_any_element()
            }
            Part::Tool(t) => {
                let (color, status, body): (Rgba, &str, String) = match &t.state {
                    ToolState::Pending(s) => (
                        warn_color(),
                        "pending",
                        serde_json::to_string(&s.input).unwrap_or_default(),
                    ),
                    ToolState::Running(s) => (
                        warn_color(),
                        "running",
                        serde_json::to_string(&s.input).unwrap_or_default(),
                    ),
                    ToolState::Completed(s) => (
                        ok_color(),
                        "completed",
                        format!(
                            "{}\n→ {}",
                            serde_json::to_string(&s.input).unwrap_or_default(),
                            truncate(s.output.trim(), 500)
                        ),
                    ),
                    ToolState::Error(s) => (bad_color(), "error", s.error.clone()),
                };
                div()
                    .my_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(soft_border())
                    .px_2()
                    .py_1()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .max_w_full()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(IconName::CircleCheck).xsmall().text_color(color))
                            .child(Label::new(format!("{} · {status}", t.tool)).text_color(color)),
                    )
                    .child(
                        Label::new(body)
                            .text_size(px(11.))
                            .text_color(muted_color()),
                    )
                    .into_any_element()
            }
            Part::Patch(p) => Label::new(format!("patch · {} file(s)", p.files.len()))
                .text_size(px(11.))
                .text_color(muted_color())
                .into_any_element(),
            other => Label::new(part_kind(other))
                .text_size(px(11.))
                .text_color(muted_color())
                .into_any_element(),
        };

        div()
            .flex()
            .items_start()
            .gap_1()
            .child(div().flex_1().min_w_0().overflow_hidden().child(content))
            .child(
                Button::new(SharedString::from(format!("inspect-{}", json.len())))
                    .label("{}")
                    .xsmall()
                    .outline()
                    .on_click(inspect),
            )
            .into_any_element()
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.store.is_busy();
        let annotating = self.annotate_target.clone();

        let send = cx.listener(|this, _: &ClickEvent, window, cx| this.send_prompt(window, cx));
        let annotate =
            cx.listener(|this, _: &ClickEvent, window, cx| this.submit_annotate(window, cx));
        let fanout = cx.listener(|this, _: &ClickEvent, window, cx| this.send_fan_out(window, cx));
        let abort = cx.listener(|this, _: &ClickEvent, _, _| this.rt.send(Command::Abort));

        div()
            .flex_none()
            .flex()
            .flex_col()
            .children(annotating.clone().map(|(sid, file, line)| {
                let short: String = sid.chars().take(8).collect();
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_1()
                    .border_t_1()
                    .border_color(warn_color())
                    .child(
                        Label::new(format!("✎ note on {file}:{line} ({short}) — type + Enter"))
                            .text_size(px(11.))
                            .text_color(warn_color()),
                    )
            }))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(div().flex_1().min_w_0().child(Input::new(&self.input)))
                    .children(if busy {
                        vec![
                            Button::new("abort")
                                .label("Abort")
                                .small()
                                .on_click(abort)
                                .into_any_element(),
                        ]
                    } else if annotating.is_some() {
                        vec![
                            Button::new("annotate-send")
                                .label("Annotate ⏎")
                                .small()
                                .on_click(annotate)
                                .into_any_element(),
                        ]
                    } else {
                        vec![
                            Button::new("send")
                                .label("Send ⏎")
                                .small()
                                .disabled(self.store.active_session.is_none())
                                .on_click(send)
                                .into_any_element(),
                            Button::new("fanout")
                                .label("⇉ all")
                                .small()
                                .disabled(
                                    !self
                                        .store
                                        .worktrees
                                        .values()
                                        .any(|r| r.session_id.is_some()),
                                )
                                .on_click(fanout)
                                .into_any_element(),
                        ]
                    }),
            )
    }
}

fn v_center(text: &str) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(Label::new(text.to_string()).text_color(muted_color()))
        .into_any_element()
}

// ------------------------------------------------------------ right panel

impl AdeApp {
    fn render_right_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = [
            (RightTab::Context, "context"),
            (RightTab::Inspector, "inspector"),
            (RightTab::Diff, "diff"),
        ];
        let mut header = div().flex().gap_1().px_2().py_1();
        for (t, name) in tabs {
            let set = cx.listener(move |app, _: &ClickEvent, _, _| {
                app.right_tab = t;
            });
            let active = self.right_tab == t;
            let label: SharedString = if active {
                format!("{name} ●").into()
            } else {
                SharedString::from(name)
            };
            header = header.child(
                Button::new(SharedString::from(format!("tab-{name}")))
                    .label(label)
                    .xsmall()
                    .compact()
                    .on_click(set),
            );
        }
        if self.right_tab == RightTab::Diff {
            let fetch_all = cx.listener(move |app, _: &ClickEvent, _, _| {
                app.rt.send(Command::FetchAllDiffs);
            });
            header = header.child(
                Button::new("diff-fetch")
                    .label("↻ all")
                    .xsmall()
                    .compact()
                    .on_click(fetch_all),
            );
        }

        let body: AnyElement = match self.right_tab {
            RightTab::Context => context_view(&self.store, cx),
            RightTab::Inspector => inspector_view(self.selected_part.as_deref()),
            RightTab::Diff => self.render_diff(cx),
        };

        div()
            .w(px(340.))
            .flex_none()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(cx.theme().border)
            .child(header)
            .child(
                div()
                    .id("right-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .py_1()
                    .child(body),
            )
    }
}

fn context_view(store: &Store, cx: &mut Context<AdeApp>) -> AnyElement {
    use ade_core::context::{SectionKind, compile};
    let view = compile(store);
    let border = cx.theme().border;

    let mut col = div().flex().flex_col().gap_1().child(
        Label::new(format!(
            "est ≈ {} tok · last step {:.0} tok",
            fmt_tok(view.est_total_tokens as f64),
            view.actual_total.unwrap_or(0.)
        ))
        .text_color(ok_color()),
    );

    for s in &view.sections {
        let color = match s.kind {
            SectionKind::System => rgba(0xb18cf0ff),
            SectionKind::User => rgba(0x5eb1f0ff),
            SectionKind::Assistant => ok_color(),
            SectionKind::Reasoning => muted_color(),
            SectionKind::ToolCall => warn_color(),
            SectionKind::Other => muted_color(),
        };
        col = col.child(
            div()
                .rounded_sm()
                .border_1()
                .border_color(border)
                .px_2()
                .py_1()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(Label::new(s.label.clone()).text_color(color))
                        .child(
                            Label::new(fmt_tok(s.est_tokens as f64))
                                .text_size(px(10.))
                                .text_color(muted_color()),
                        ),
                )
                .children((!s.detail.is_empty()).then(|| {
                    Label::new(truncate(&s.detail, 260))
                        .text_size(px(11.))
                        .text_color(muted_color())
                })),
        );
    }
    col.into_any_element()
}

fn inspector_view(selected: Option<&str>) -> AnyElement {
    match selected {
        None => v_center("Click {} on any part of the timeline."),
        Some(json) => {
            let mut lines = div().flex().flex_col();
            for line in json.lines().take(500) {
                lines = lines.child(Label::new(line.to_string()).text_size(px(11.)));
            }
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(Label::new("Raw part"))
                .child(lines)
                .into_any_element()
        }
    }
}

#[derive(serde::Deserialize)]
struct FileDiffRow {
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    additions: Option<f64>,
    #[serde(default)]
    deletions: Option<f64>,
    #[serde(default)]
    patch: Option<String>,
}

impl AdeApp {
    fn file_diff_block(
        &self,
        sid: &str,
        file_idx: usize,
        d: &FileDiffRow,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let name = d.file.clone().unwrap_or_else(|| "(unknown)".into());
        let mut block = div().flex().flex_col().gap_0p5().child(Label::new(format!(
            "{name}  +{} −{}",
            d.additions.unwrap_or(0.),
            d.deletions.unwrap_or(0.)
        )));
        if let Some(patch) = d.patch.clone() {
            let mut lines = div().flex().flex_col();
            for (li, pl) in parse_patch_lines(&patch).iter().take(400).enumerate() {
                let color = if pl.text.starts_with('+') && !pl.text.starts_with("+++") {
                    ok_color()
                } else if pl.text.starts_with('-') && !pl.text.starts_with("---") {
                    bad_color()
                } else if pl.text.starts_with("@@") {
                    rgba(0xb18cf0ff)
                } else {
                    muted_color()
                };
                let row = div().child(Label::new(pl.text.clone()).text_color(color));
                match pl.line {
                    Some(n) => {
                        let target = (sid.to_string(), name.clone(), n);
                        let set = cx.listener(move |app, _: &ClickEvent, _, cx| {
                            app.annotate_target = Some(target.clone());
                            cx.notify();
                        });
                        lines = lines.child(
                            row.id(SharedString::from(format!("dl-{sid}-{file_idx}-{li}")))
                                .cursor_pointer()
                                .on_click(set),
                        );
                    }
                    None => {
                        lines = lines.child(row);
                    }
                }
            }
            block = block.child(lines);
        }
        // Notes attached to this file.
        for (idx, note) in self
            .diff_notes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.session_id == sid && n.file == name)
        {
            let drop = cx.listener(move |app, _: &ClickEvent, _, cx| {
                app.diff_notes.remove(idx);
                cx.notify();
            });
            block = block.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl_2()
                    .child(
                        Label::new(format!("✎ L{}: {}", note.line, truncate(&note.text, 120)))
                            .text_size(px(11.))
                            .text_color(warn_color()),
                    )
                    .child(
                        Button::new(SharedString::from(format!("note-del-{idx}")))
                            .label("×")
                            .xsmall()
                            .compact()
                            .on_click(drop),
                    ),
            );
        }
        block
    }

    fn render_diff(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.store.diffs.is_empty() {
            return v_center("No diffs yet — press ↻ all to fetch.");
        }
        // Group sessions with diffs by scope: worktrees first, then root.
        let mut scopes: Vec<String> = self
            .store
            .diffs
            .keys()
            .map(|sid| self.store.scope_of(sid).to_string())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        scopes.sort_by_key(|s| (!s.is_empty(), s.clone()));

        let mut col = div().flex().flex_col().gap_2();
        for scope in scopes {
            let header = if scope.is_empty() {
                "main".to_string()
            } else {
                format!("⑂ {scope}")
            };
            let mut head_row = div().flex().items_center().justify_between().child(
                Label::new(header)
                    .text_size(px(12.))
                    .text_color(warn_color()),
            );
            if !scope.is_empty() {
                let merge_slug = scope.clone();
                let merge = cx.listener(move |app, _: &ClickEvent, _, _| {
                    app.rt.send(Command::MergeWorktree {
                        slug: merge_slug.clone(),
                    });
                });
                head_row = head_row.child(
                    Button::new(SharedString::from(format!("merge-{scope}")))
                        .label("Merge winner")
                        .xsmall()
                        .compact()
                        .on_click(merge),
                );
            }
            let mut section = div().flex().flex_col().gap_1().child(head_row);
            let mut sids: Vec<_> = self
                .store
                .diffs
                .keys()
                .filter(|sid| self.store.scope_of(sid) == scope)
                .cloned()
                .collect();
            sids.sort();
            for sid in sids {
                let notes: Vec<DiffNote> = self
                    .diff_notes
                    .iter()
                    .filter(|n| n.session_id == sid)
                    .cloned()
                    .collect();
                let short: String = sid.chars().take(12).collect();
                let mut head = div().flex().items_center().justify_between().child(
                    Label::new(short.to_string())
                        .text_size(px(11.))
                        .text_color(muted_color()),
                );
                if !notes.is_empty() {
                    let send_sid = sid.clone();
                    let send_notes = cx.listener(move |app, _: &ClickEvent, _, _| {
                        let mine: Vec<DiffNote> = app
                            .diff_notes
                            .iter()
                            .filter(|n| n.session_id == send_sid)
                            .cloned()
                            .collect();
                        app.diff_notes.retain(|n| n.session_id != send_sid);
                        app.rt.send(Command::SendNotes {
                            session_id: send_sid.clone(),
                            notes: mine,
                        });
                    });
                    head = head.child(
                        Button::new(SharedString::from(format!("notes-send-{short}")))
                            .label(format!("Send {} notes → agent", notes.len()))
                            .xsmall()
                            .compact()
                            .on_click(send_notes),
                    );
                }
                section = section.child(head);
                let Some(value) = self.store.diffs.get(&sid) else {
                    continue;
                };
                let Ok(rows) = serde_json::from_value::<Vec<FileDiffRow>>(value.clone()) else {
                    continue;
                };
                section = section.child(
                    Label::new(format!("{} file(s) — click a line to annotate", rows.len()))
                        .text_size(px(11.))
                        .text_color(muted_color()),
                );
                for (fi, d) in rows.iter().enumerate() {
                    section = section.child(self.file_diff_block(&sid, fi, d, cx));
                }
            }
            col = col.child(section);
        }
        col.into_any_element()
    }
}

// ------------------------------------------------------------ permission

impl AdeApp {
    fn render_permission_overlay(
        &self,
        p: ade_core::PendingPermission,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let once_pid = p.permission_id.clone();
        let always_pid = p.permission_id.clone();
        let reject_pid = p.permission_id.clone();
        let once = cx.listener(move |app, _: &ClickEvent, _, _| {
            app.rt.send(Command::PermissionReply {
                permission_id: once_pid.clone(),
                response: PermissionResponse::Once,
            });
        });
        let always = cx.listener(move |app, _: &ClickEvent, _, _| {
            app.rt.send(Command::PermissionReply {
                permission_id: always_pid.clone(),
                response: PermissionResponse::Always,
            });
        });
        let reject = cx.listener(move |app, _: &ClickEvent, _, _| {
            app.rt.send(Command::PermissionReply {
                permission_id: reject_pid.clone(),
                response: PermissionResponse::Reject,
            });
        });

        let command = p
            .metadata
            .get("command")
            .and_then(|c| c.as_str())
            .map(str::to_string);

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(120.))
            .bg(rgba(0x00000099))
            .child(
                div()
                    .w(px(420.))
                    .rounded_lg()
                    .border_1()
                    .border_color(warn_color())
                    .bg(rgb(0x1d2029))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(Label::new("🔒 Permission required").text_size(px(15.)))
                    .child(Label::new(p.kind.clone()).text_color(warn_color()))
                    .children(command.map(|c| Label::new(c).text_color(muted_color())))
                    .children((!p.patterns.is_empty()).then(|| {
                        Label::new(truncate(&p.patterns.join(", "), 120))
                            .text_size(px(11.))
                            .text_color(muted_color())
                    }))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .pt_1()
                            .child(
                                Button::new("perm-once")
                                    .label("Allow once")
                                    .small()
                                    .on_click(once),
                            )
                            .child(
                                Button::new("perm-always")
                                    .label("Always allow")
                                    .small()
                                    .on_click(always),
                            )
                            .child(
                                Button::new("perm-reject")
                                    .label("Reject")
                                    .small()
                                    .on_click(reject),
                            ),
                    ),
            )
    }
}

// ---------------------------------------------------------------- helpers

fn part_kind(p: &Part) -> &'static str {
    match p {
        Part::Subtask(_) => "[subtask]",
        Part::File(_) => "[file]",
        Part::Snapshot(_) => "[snapshot]",
        Part::Agent(_) => "[agent]",
        Part::Retry(_) => "[retry]",
        Part::Compaction(_) => "[compaction]",
        _ => "[part]",
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max_chars).collect::<String>())
    }
}

fn fmt_tok(n: f64) -> String {
    if n >= 10_000. {
        format!("{:.1}k", n / 1000.)
    } else {
        format!("{n:.0}")
    }
}
