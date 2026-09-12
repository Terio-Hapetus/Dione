use ade_core::{Command, WorktreeStatus};
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, button::Button, label::Label,
};

use super::theme::{bad_color, ok_color, truncate, warn_color};
use crate::app::AdeApp;

impl AdeApp {
    pub(crate) fn session_dot(&self, id: &str) -> Option<Rgba> {
        if self.store.has_pending(id) {
            return Some(warn_color());
        }
        if self.store.is_working(id) {
            return Some(ok_color());
        }
        None
    }

    pub(crate) fn worktree_dot(&self, slug: &str) -> Option<Rgba> {
        match self.store.worktree_status(slug) {
            WorktreeStatus::Working => Some(ok_color()),
            WorktreeStatus::NeedsYou => Some(warn_color()),
            WorktreeStatus::Creating | WorktreeStatus::Done => None,
        }
    }

    pub(crate) fn session_row(
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

    pub(crate) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                if let Some(record) = this.store.worktrees.get(&remove_slug) {
                    this.vm.stop(remove_slug.clone(), record.path.clone());
                }
                this.rt.send(Command::RemoveWorktree {
                    slug: remove_slug.clone(),
                });
            });
            let open_slug = slug.clone();
            let open = cx.listener(move |this, _: &ClickEvent, _, _| {
                if let Some(record) = this.store.worktrees.get(&open_slug) {
                    this.vm.ensure(open_slug.clone(), record.path.clone());
                }
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
                                .child(Label::new(format!("⑂ {slug}")).text_size(px(12.)))
                                .children(self.vm_badge(&slug)),
                        )
                        .child(
                            Button::new(SharedString::from(format!("wt-open-{slug}")))
                                .label("⏻")
                                .xsmall()
                                .compact()
                                .on_click(open),
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

    pub(crate) fn render_error_strip(&self) -> impl IntoElement {
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
