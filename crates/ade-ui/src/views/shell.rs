//! UX1 IDE shell: activity rail (48px) + status bar (24px).
//!
//! The rail switches *views*, not data: Fleet = sidebar + chat,
//! Review = sidebar + diff tab, Terminal = sidebar + terminal.
//! Active highlight is derived from `show_terminal`/`right_tab`
//! so no extra state can desync.

use gpui::*;
use gpui_component::{ActiveTheme as _, Sizable as _, button::Button, label::Label};

use super::theme::{STATUS_H, TEXT_META, muted_color, status_glyph, truncate};
use crate::app::{AdeApp, RightTab};

impl AdeApp {
    /// 48px icon rail: Fleet / Review / Terminal.
    pub(crate) fn render_activity_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let to_fleet = cx.listener(|app, _: &ClickEvent, _, cx| {
            app.show_terminal = false;
            cx.notify();
        });
        let to_review = cx.listener(|app, _: &ClickEvent, _, cx| {
            app.show_terminal = false;
            app.right_tab = RightTab::Diff;
            cx.notify();
        });
        let to_term = cx.listener(|app, _: &ClickEvent, _, _| {
            app.toggle_terminal();
        });
        let fleet_active = !self.show_terminal && self.right_tab != RightTab::Diff;
        let review_active = !self.show_terminal && self.right_tab == RightTab::Diff;
        let term_active = self.show_terminal;
        let mut fleet_btn = Button::new(SharedString::from("act-fleet".to_string()))
            .label("⑂")
            .small();
        if fleet_active {
            fleet_btn = fleet_btn.outline();
        }
        let mut review_btn = Button::new(SharedString::from("act-review".to_string()))
            .label("Δ")
            .small();
        if review_active {
            review_btn = review_btn.outline();
        }
        let mut term_btn = Button::new(SharedString::from("act-term".to_string()))
            .label(">_")
            .small();
        if term_active {
            term_btn = term_btn.outline();
        }
        div()
            .w(px(super::theme::ACTIVITY_W))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .py_2()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(fleet_btn.on_click(to_fleet))
            .child(review_btn.on_click(to_review))
            .child(term_btn.on_click(to_term))
    }

    /// 24px bottom bar: host/vm mode · counts left, last error right.
    /// Read-only — retry lives in the sidebar row / error strip (M7b).
    /// Fleet glyph summarizes the worst state (blocked > needs-you >
    /// working), same priority as the sidebar dots.
    pub(crate) fn render_status_bar(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let mode = if self.vm_available {
            "host"
        } else {
            "host-only"
        };
        let mut blocked = false;
        let mut needs_you = false;
        let mut working = false;
        for slug in self.store.worktrees.keys() {
            if self.store.is_blocked_slug(slug) {
                blocked = true;
            }
            match self.store.worktree_status(slug) {
                ade_core::WorktreeStatus::NeedsYou => needs_you = true,
                ade_core::WorktreeStatus::Working => working = true,
                _ => {}
            }
        }
        let glyph = status_glyph(working, needs_you, blocked, false);
        let left = format!(
            "{glyph} {mode} · {} wt · {} ses",
            self.store.worktrees.len(),
            self.store.sessions.len()
        );
        let right = self
            .store
            .errors
            .back()
            .map(|e| format!("⚠ {}", truncate(e, 120)))
            .unwrap_or_else(|| "ready".to_string());
        div()
            .h(px(STATUS_H))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .border_t_1()
            .border_color(super::theme::soft_border())
            .child(
                Label::new(left)
                    .text_size(px(TEXT_META))
                    .text_color(muted_color()),
            )
            .child(
                Label::new(right)
                    .text_size(px(TEXT_META))
                    .text_color(muted_color()),
            )
    }
}
