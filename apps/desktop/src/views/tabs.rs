//! UX3 worktree tab bar: one tab per worktree + `main`.
//!
//! Clicking a tab selects the worktree (same `Command::SelectWorktree`
//! as the sidebar row); the `main` tab selects the first root session.
//! Badges reuse the Fleet glyph so sidebar and tabs always agree.

use base::{Command, WorktreeStatus};
use gpui::*;
use gpui_component::{ActiveTheme as _, Sizable as _, button::Button, label::Label};

use super::sidebar::fleet_rank;
use super::theme::{TEXT_SECONDARY, muted_for, status_glyph};
use crate::app::DioneApp;

impl DioneApp {
    /// Tab bar pinned above the chat/terminal column (UX3).
    pub(crate) fn render_worktree_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = cx.theme().is_dark();
        let mut row = div()
            .id("wt-tabs")
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .overflow_x_scroll()
            .border_b_1()
            .border_color(cx.theme().border);

        // `main` tab: visible when root sessions exist.
        let roots: Vec<String> = self.store.sessions_in_scope("").into_iter().collect();
        if let Some(first) = roots.first() {
            let active = self.store.active_worktree.is_none();
            let select = first.clone();
            let on_click = cx.listener(move |this, _: &ClickEvent, _, _| {
                this.rt.send(Command::SelectSession { id: select.clone() });
            });
            let mut btn = Button::new("wt-tab-main").label("main").xsmall();
            if active {
                btn = btn.outline();
            }
            row = row.child(btn.on_click(on_click));
        }

        // Worktree tabs, same attention order as the Fleet sidebar.
        let mut slugs: Vec<_> = self.store.worktrees.keys().cloned().collect();
        slugs.sort_by_key(|s| {
            (
                fleet_rank(self.store.is_blocked_slug(s), self.store.worktree_status(s)),
                s.clone(),
            )
        });
        for slug in slugs {
            let status = self.store.worktree_status(&slug);
            let blocked = self.store.is_blocked_slug(&slug);
            let glyph = status_glyph(
                status == WorktreeStatus::Working,
                status == WorktreeStatus::NeedsYou,
                blocked,
                status == WorktreeStatus::Done,
            );
            let active = self.store.active_worktree.as_deref() == Some(slug.as_str());
            let select = slug.clone();
            let on_click = cx.listener(move |this, _: &ClickEvent, _, _| {
                this.rt.send(Command::SelectWorktree {
                    slug: select.clone(),
                });
            });
            let mut btn = Button::new(SharedString::from(format!("wt-tab-{slug}")))
                .label(format!("{glyph} {slug}"))
                .xsmall();
            if active {
                btn = btn.outline();
            }
            row = row.child(btn.on_click(on_click));
        }

        row.child(
            Label::new(format!("{} chats", self.store.sessions.len()))
                .text_size(px(TEXT_SECONDARY))
                .text_color(muted_for(dark)),
        )
    }
}
