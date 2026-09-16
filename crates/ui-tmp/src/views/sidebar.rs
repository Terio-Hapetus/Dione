use base::{Command, TaskId, WorktreeStatus};
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, button::Button, input::Input, label::Label,
};

use super::theme::{
    ROW_H, SIDEBAR_W, TEXT_META, TEXT_SECONDARY, active_bg, bad_color, empty_state, muted_for,
    ok_color, status_glyph, truncate, warn_color,
};
use crate::app::AdeApp;

/// Fleet sort rank (pure: unit-tested). Blocked first, then needs-you,
/// then working; idle/done sink to the bottom, alphabetical inside a rank.
pub(crate) fn fleet_rank(blocked: bool, status: WorktreeStatus) -> u8 {
    if blocked {
        0
    } else {
        match status {
            WorktreeStatus::NeedsYou => 1,
            WorktreeStatus::Working => 2,
            WorktreeStatus::Creating | WorktreeStatus::Done => 3,
        }
    }
}

/// Dot for a worktree row (pure: unit-tested). Blocked (retry budget
/// spent) overrides every session-derived state with red.
pub(crate) fn fleet_dot(status: WorktreeStatus, blocked: bool) -> Option<Rgba> {
    if blocked {
        return Some(bad_color());
    }
    match status {
        WorktreeStatus::Working => Some(ok_color()),
        WorktreeStatus::NeedsYou => Some(warn_color()),
        WorktreeStatus::Creating | WorktreeStatus::Done => None,
    }
}

/// Task id behind a `fleet: blocked {id} …` sweep message (pure:
/// unit-tested). Anything else → None, never panics.
pub(crate) fn blocked_task_in(msg: &str) -> Option<TaskId> {
    msg.strip_prefix("fleet: blocked ")?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

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
        fleet_dot(
            self.store.worktree_status(slug),
            self.store.is_blocked_slug(slug),
        )
    }

    pub(crate) fn session_row(
        &self,
        id: &str,
        title: String,
        indent: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.store.active_session.as_deref() == Some(id);
        let dark = cx.theme().is_dark();
        let dot = self.session_dot(id);
        let row_id = SharedString::from(format!("ses-row-{id}"));
        let select_id = id.to_string();
        let bg: Hsla = if active {
            active_bg(dark).into()
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
            .h(px(ROW_H))
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
        let dark = cx.theme().is_dark();
        let new_session = cx.listener(|this, _: &ClickEvent, _, _| {
            this.rt.send(Command::CreateSession {
                title: String::new(),
            });
        });
        // "+ wt" opens the Fleet dialog (UX2) — the composer text is
        // never stolen for a slug anymore.
        let open_wt_dialog = cx.listener(|this, _: &ClickEvent, _, cx| {
            this.show_wt_dialog = true;
            cx.notify();
        });
        let toggle_attention = cx.listener(|this, _: &ClickEvent, _, cx| {
            this.fleet_attention_only = !this.fleet_attention_only;
            cx.notify();
        });

        let mut list = div()
            .id("sessions")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();

        // New-worktree dialog: slug input + Create/Cancel (Enter submits).
        if self.show_wt_dialog {
            let create = cx.listener(|this, _: &ClickEvent, window, cx| {
                this.create_worktree_from_dialog(window, cx);
            });
            let cancel = cx.listener(|this, _: &ClickEvent, _, cx| {
                this.show_wt_dialog = false;
                cx.notify();
            });
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .m_2()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        Label::new("New worktree — 1 task = 1 worktree")
                            .text_size(px(TEXT_META))
                            .text_color(muted_for(dark)),
                    )
                    .child(Input::new(&self.fleet_input))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                Button::new("wt-create")
                                    .label("Create")
                                    .small()
                                    .on_click(create),
                            )
                            .child(
                                Button::new("wt-cancel")
                                    .label("Cancel")
                                    .small()
                                    .on_click(cancel),
                            ),
                    ),
            );
        }

        // Worktree groups, attention-first (blocked → needs-you →
        // working), alphabetical inside a rank.
        let mut slugs: Vec<_> = self.store.worktrees.keys().cloned().collect();
        slugs.sort_by_key(|s| {
            (
                fleet_rank(self.store.is_blocked_slug(s), self.store.worktree_status(s)),
                s.clone(),
            )
        });
        if self.fleet_attention_only {
            slugs.retain(|s| {
                fleet_rank(self.store.is_blocked_slug(s), self.store.worktree_status(s)) < 3
            });
        }
        let shown_slugs = slugs.len();
        for slug in slugs {
            let active = self.store.active_worktree.as_deref() == Some(slug.as_str());
            let status = self.store.worktree_status(&slug);
            let blocked = self.store.is_blocked_slug(&slug);
            let glyph = status_glyph(
                status == WorktreeStatus::Working,
                status == WorktreeStatus::NeedsYou,
                blocked,
                status == WorktreeStatus::Done,
            );
            let color = self.worktree_dot(&slug).unwrap_or_else(|| muted_for(dark));
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
                active_bg(dark).into()
            } else {
                Hsla::transparent_black()
            };
            list = list.child(
                div()
                    .id(SharedString::from(format!("wt-row-{slug}")))
                    .h(px(ROW_H))
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
                            .child(Label::new(glyph).text_color(color))
                            .child(Label::new(format!("⑂ {slug}")).text_size(px(TEXT_SECONDARY)))
                            .children(self.vm_badge(&slug, dark)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .children(blocked.then(|| {
                                let retry_slug = slug.clone();
                                let retry = cx.listener(move |this, _: &ClickEvent, _, _| {
                                    this.rt.send(Command::RetryTask {
                                        slug: retry_slug.clone(),
                                    });
                                });
                                Button::new(SharedString::from(format!("wt-retry-{slug}")))
                                    .label("↻")
                                    .xsmall()
                                    .compact()
                                    .on_click(retry)
                            }))
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
        let mut root_count = 0;
        for sid in self.store.sessions_in_scope("") {
            let title = self
                .store
                .sessions
                .get(&sid)
                .map(|s| truncate(&s.title, 24))
                .unwrap_or_else(|| "(gone)".into());
            list = list.child(self.session_row(&sid, title, false, cx));
            root_count += 1;
        }
        if shown_slugs == 0 && root_count == 0 && !self.show_wt_dialog {
            list = list.child(empty_state(
                "⑂",
                "No worktrees yet",
                if self.fleet_attention_only {
                    "Nothing needs you — toggle ! to see all"
                } else {
                    "Press + wt to open your first task"
                },
                dark,
            ));
        }

        let filter_label = if self.fleet_attention_only {
            "!"
        } else {
            "all"
        };

        div()
            .w(px(SIDEBAR_W))
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
                                Button::new("fleet-filter")
                                    .label(filter_label)
                                    .xsmall()
                                    .compact()
                                    .on_click(toggle_attention),
                            )
                            .child(
                                Button::new("wt-new")
                                    .label("+ wt")
                                    .xsmall()
                                    .compact()
                                    .on_click(open_wt_dialog),
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

    pub(crate) fn render_error_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let last = self.store.errors.back().cloned();
        // Retry target behind the newest blocked message, if still blocked
        // (a retried/cleared task hides the button by itself).
        let retry_slug = last
            .as_deref()
            .and_then(blocked_task_in)
            .and_then(|id| self.store.blocked.get(&id).cloned());
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
                .children(retry_slug.map(|slug| {
                    let retry = cx.listener(move |this, _: &ClickEvent, _, _| {
                        this.rt.send(Command::RetryTask { slug: slug.clone() });
                    });
                    Button::new("err-retry")
                        .label("↻ retry")
                        .xsmall()
                        .compact()
                        .on_click(retry)
                }))
        }))
    }
}

#[cfg(test)]
mod tests {
    // NOTE: same pitfall as vm_badge — no `use super::*`; the file's
    // `use gpui::*` glob would shadow builtin `#[test]`.
    use base::WorktreeStatus;

    use crate::views::sidebar::{blocked_task_in, fleet_dot, fleet_rank};
    use crate::views::theme::{bad_color, ok_color, warn_color};

    #[test]
    fn attention_sorts_before_idle() {
        // Blocked > needs-you > working > idle/done.
        assert!(
            fleet_rank(true, WorktreeStatus::Done) < fleet_rank(false, WorktreeStatus::NeedsYou)
        );
        assert!(
            fleet_rank(false, WorktreeStatus::NeedsYou)
                < fleet_rank(false, WorktreeStatus::Working)
        );
        assert!(
            fleet_rank(false, WorktreeStatus::Working)
                < fleet_rank(false, WorktreeStatus::Creating)
        );
        assert_eq!(
            fleet_rank(false, WorktreeStatus::Creating),
            fleet_rank(false, WorktreeStatus::Done)
        );
    }

    #[test]
    fn blocked_overrides_every_dot() {
        for status in [
            WorktreeStatus::Creating,
            WorktreeStatus::Working,
            WorktreeStatus::NeedsYou,
            WorktreeStatus::Done,
        ] {
            assert_eq!(fleet_dot(status, true), Some(bad_color()));
        }
    }

    #[test]
    fn unblocked_keeps_legacy_dots() {
        assert_eq!(fleet_dot(WorktreeStatus::Working, false), Some(ok_color()));
        assert_eq!(
            fleet_dot(WorktreeStatus::NeedsYou, false),
            Some(warn_color())
        );
        assert_eq!(fleet_dot(WorktreeStatus::Creating, false), None);
        assert_eq!(fleet_dot(WorktreeStatus::Done, false), None);
    }

    #[test]
    fn blocked_id_parses_from_sweep_message() {
        use base::TaskId;

        let id = TaskId::new();
        let msg = format!("fleet: blocked {id} (retry budget spent, needs a human look)");
        assert_eq!(blocked_task_in(&msg), Some(id));
        // Reclaim lines, foreign errors, and garbage never match.
        assert_eq!(
            blocked_task_in("fleet: reclaim x (overdue/stale, will retry)"),
            None
        );
        assert_eq!(blocked_task_in("worktree client failed: boom"), None);
        assert_eq!(blocked_task_in("fleet: blocked not-a-uuid (x)"), None);
        assert_eq!(blocked_task_in(""), None);
    }
}
