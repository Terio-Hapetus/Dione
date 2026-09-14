//! UX6 command palette: one searchable surface for every primary action.
//!
//! Opened from the top-bar `⌘K` button (global keymaps wait for a GPUI
//! keymap pass — the palette is mouse+type driven like the UX2 dialog).
//! Typing filters, Enter runs the first match, clicking the backdrop or
//! `×` closes. Actions reuse existing `Command`s — no new runtime paths.

use std::collections::BTreeSet;

use ade_core::Command;
use gpui::*;
use gpui_component::{ActiveTheme as _, Sizable as _, button::Button, input::Input, label::Label};

use super::sidebar::fleet_rank;
use super::theme::{TEXT_META, empty_state, muted_color};
use crate::app::{AdeApp, RightTab};

/// A runnable palette entry. Labels carry the matching text; `hint`
/// names the group (go / review / fleet / view).
pub(crate) struct PaletteItem {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) hint: String,
    pub(crate) action: PaletteAction,
}

#[derive(Clone)]
pub(crate) enum PaletteAction {
    SelectWorktree(String),
    SelectSession(String),
    FetchAllDiffs,
    RetryTask(String),
    MergeWorktree(String),
    OpenWtDialog,
    ToggleTerminal,
    SetRightTab(RightTab),
}

/// Case-insensitive substring filter (pure: unit-tested). Empty query
/// matches everything; caps at `limit` so the overlay stays short.
pub(crate) fn palette_match<'a>(
    query: &str,
    items: &'a [PaletteItem],
    limit: usize,
) -> Vec<&'a PaletteItem> {
    let q = query.trim().to_lowercase();
    items
        .iter()
        .filter(|it| q.is_empty() || it.label.to_lowercase().contains(&q))
        .take(limit)
        .collect()
}

impl AdeApp {
    /// Every primary action, grouped. Worktree entries follow the same
    /// attention order as the sidebar/tabs.
    pub(crate) fn palette_items(&self) -> Vec<PaletteItem> {
        let mut items = Vec::new();
        let mut slugs: Vec<_> = self.store.worktrees.keys().cloned().collect();
        slugs.sort_by_key(|s| {
            (
                fleet_rank(self.store.is_blocked_slug(s), self.store.worktree_status(s)),
                s.clone(),
            )
        });
        for slug in &slugs {
            items.push(PaletteItem {
                id: format!("go-{slug}"),
                label: format!("Go to ⑂ {slug}"),
                hint: "go".into(),
                action: PaletteAction::SelectWorktree(slug.clone()),
            });
        }
        for sid in self.store.sessions_in_scope("") {
            let title = self
                .store
                .sessions
                .get(&sid)
                .map(|s| s.title.clone())
                .unwrap_or_else(|| "(gone)".into());
            items.push(PaletteItem {
                id: format!("go-{sid}"),
                label: format!("Go to {title}"),
                hint: "go".into(),
                action: PaletteAction::SelectSession(sid),
            });
        }
        for slug in &slugs {
            if self.store.is_blocked_slug(slug) {
                items.push(PaletteItem {
                    id: format!("retry-{slug}"),
                    label: format!("Retry blocked ⑂ {slug}"),
                    hint: "fleet".into(),
                    action: PaletteAction::RetryTask(slug.clone()),
                });
            }
        }
        // Merge winners only for scopes that actually have diffs.
        let diff_scopes: BTreeSet<String> = self
            .store
            .diffs
            .keys()
            .map(|sid| self.store.scope_of(sid).to_string())
            .collect();
        for slug in &slugs {
            if diff_scopes.contains(slug) {
                items.push(PaletteItem {
                    id: format!("merge-{slug}"),
                    label: format!("Merge winner ⑂ {slug}"),
                    hint: "review".into(),
                    action: PaletteAction::MergeWorktree(slug.clone()),
                });
            }
        }
        items.push(PaletteItem {
            id: "fetch-diffs".into(),
            label: "Fetch all diffs".into(),
            hint: "review".into(),
            action: PaletteAction::FetchAllDiffs,
        });
        items.push(PaletteItem {
            id: "new-wt".into(),
            label: "New worktree…".into(),
            hint: "fleet".into(),
            action: PaletteAction::OpenWtDialog,
        });
        items.push(PaletteItem {
            id: "toggle-term".into(),
            label: "Toggle terminal".into(),
            hint: "view".into(),
            action: PaletteAction::ToggleTerminal,
        });
        for (tab, name) in [
            (RightTab::Context, "context"),
            (RightTab::Diff, "diff"),
            (RightTab::File, "file"),
        ] {
            items.push(PaletteItem {
                id: format!("view-{name}"),
                label: format!("Show {name} panel"),
                hint: "view".into(),
                action: PaletteAction::SetRightTab(tab),
            });
        }
        items
    }

    /// Run an action and close the palette.
    pub(crate) fn run_palette(&mut self, action: PaletteAction, cx: &mut Context<Self>) {
        self.show_palette = false;
        match action {
            PaletteAction::SelectWorktree(slug) => {
                self.rt.send(Command::SelectWorktree { slug });
            }
            PaletteAction::SelectSession(id) => {
                self.rt.send(Command::SelectSession { id });
            }
            PaletteAction::FetchAllDiffs => {
                self.rt.send(Command::FetchAllDiffs);
            }
            PaletteAction::RetryTask(slug) => {
                self.rt.send(Command::RetryTask { slug });
            }
            PaletteAction::MergeWorktree(slug) => {
                self.rt.send(Command::MergeWorktree { slug });
            }
            PaletteAction::OpenWtDialog => {
                self.show_wt_dialog = true;
            }
            PaletteAction::ToggleTerminal => {
                self.toggle_terminal();
            }
            PaletteAction::SetRightTab(tab) => {
                self.right_tab = tab;
            }
        }
        cx.notify();
    }

    /// Run the first filtered match (palette input Enter).
    pub(crate) fn run_palette_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.palette_input.read(cx).value().to_string();
        let items = self.palette_items();
        if let Some(first) = palette_match(&query, &items, 12).first() {
            let action = first.action.clone();
            self.palette_input
                .update(cx, |st, cx| st.set_value("", window, cx));
            self.run_palette(action, cx);
        }
    }

    /// Centered overlay: input + up to 12 matches + backdrop-click close.
    pub(crate) fn render_palette(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.palette_input.read(cx).value().to_string();
        let items = self.palette_items();
        let matched = palette_match(&query, &items, 12);
        let mut list = div().flex().flex_col().gap_0p5();
        if matched.is_empty() {
            list = list.child(empty_state(
                "○",
                "No match",
                "try a worktree name or action",
            ));
        }
        for it in matched {
            let action = it.action.clone();
            let run = cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.run_palette(action.clone(), cx);
            });
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        Button::new(SharedString::from(it.id.clone()))
                            .label(it.label.clone())
                            .xsmall()
                            .on_click(run),
                    )
                    .child(
                        Label::new(it.hint.clone())
                            .text_size(px(TEXT_META))
                            .text_color(muted_color()),
                    ),
            );
        }
        let close_bg = cx.listener(|this, _: &ClickEvent, _, cx| {
            this.show_palette = false;
            cx.notify();
        });
        let close_btn = cx.listener(|this, _: &ClickEvent, _, cx| {
            this.show_palette = false;
            cx.notify();
        });
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .id("palette-bg")
                    .on_click(close_bg),
            )
            .child(
                div().flex().justify_center().pt_20().child(
                    div()
                        .id("palette-panel")
                        .w(px(480.))
                        .max_h(px(420.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_2()
                        .rounded_md()
                        .bg(cx.theme().background)
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(Input::new(&self.palette_input))
                        .child(
                            Label::new("type to filter · Enter runs first · click × to close")
                                .text_size(px(TEXT_META))
                                .text_color(muted_color()),
                        )
                        .child(list)
                        .child(
                            Button::new("palette-close")
                                .label("× close")
                                .xsmall()
                                .on_click(close_btn),
                        ),
                ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use crate::views::palette::{PaletteAction, PaletteItem, palette_match};

    fn item(id: &str, label: &str) -> PaletteItem {
        PaletteItem {
            id: id.to_string(),
            label: label.to_string(),
            hint: "go".to_string(),
            action: PaletteAction::FetchAllDiffs,
        }
    }

    #[test]
    fn filter_is_case_insensitive_and_capped() {
        let items = vec![
            item("a", "Go to ⑂ auth-fix"),
            item("b", "Go to ⑂ AUTH-RETRY"),
            item("c", "Fetch all diffs"),
        ];
        assert_eq!(palette_match("", &items, 10).len(), 3);
        assert_eq!(palette_match("auth", &items, 10).len(), 2);
        assert_eq!(palette_match("zzz", &items, 10).len(), 0);
        assert_eq!(palette_match("", &items, 2).len(), 2);
    }

    #[test]
    fn empty_query_lists_everything() {
        let items = vec![item("a", "Go to ⑂ x"), item("b", "Fetch all diffs")];
        assert_eq!(palette_match("", &items, 10).len(), 2);
        assert_eq!(palette_match("  ", &items, 10).len(), 2);
    }
}
