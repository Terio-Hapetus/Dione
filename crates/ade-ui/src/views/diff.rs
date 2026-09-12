use ade_core::{Command, DiffNote, parse_patch_lines, worktree::split_hunks};
use gpui::*;
use gpui_component::{Sizable as _, button::Button, label::Label};

use super::theme::{bad_color, muted_color, ok_color, truncate, v_center, warn_color};
use crate::app::AdeApp;

#[derive(serde::Deserialize)]
pub(crate) struct FileDiffRow {
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
    pub(crate) fn file_diff_block(
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
            // Track which @@ hunk each rendered row belongs to so @@ rows
            // get a cherry-pick toggle (M8a). Preamble rows get None.
            let mut hunk_idx: Option<usize> = None;
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
                if pl.text.starts_with("@@") {
                    hunk_idx = Some(hunk_idx.map_or(0, |i| i + 1));
                }
                let row = div().child(Label::new(pl.text.clone()).text_color(color));
                // @@ rows carry a pick toggle; +/- rows keep annotate clicks.
                if let Some(hi) = hunk_idx.filter(|_| pl.text.starts_with("@@")) {
                    let key = (sid.to_string(), name.clone(), hi);
                    let picked = self.hunk_picks.contains(&key);
                    let key_toggle = key.clone();
                    let toggle = cx.listener(move |app, _: &ClickEvent, _, cx| {
                        if !app.hunk_picks.remove(&key_toggle) {
                            app.hunk_picks.insert(key_toggle.clone());
                        }
                        cx.notify();
                    });
                    lines = lines.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new(SharedString::from(format!(
                                    "hunk-{sid}-{file_idx}-{hi}"
                                )))
                                .label(if picked { "☑" } else { "☐" })
                                .xsmall()
                                .compact()
                                .on_click(toggle),
                            )
                            .child(Label::new(pl.text.clone()).text_color(color)),
                    );
                    continue;
                }
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
            // Cherry-pick (M8a): apply picked hunks of this file to the
            // main checkout. Picks clear on send.
            let picked: Vec<usize> = self
                .hunk_picks
                .iter()
                .filter(|(s, f, _)| s == sid && *f == name)
                .map(|(_, _, i)| *i)
                .collect();
            if !picked.is_empty() {
                let n = picked.len();
                let apply_sid = sid.to_string();
                let apply_file = name.clone();
                let apply_patch = patch.clone();
                let apply = cx.listener(move |app, _: &ClickEvent, _, cx| {
                    let all = split_hunks(&apply_patch);
                    let hunks = picked
                        .iter()
                        .filter_map(|i| all.get(*i).cloned())
                        .collect::<Vec<_>>();
                    app.hunk_picks
                        .retain(|(s, f, _)| !(s == &apply_sid && *f == apply_file));
                    app.rt.send(Command::ApplyHunks {
                        file: apply_file.clone(),
                        hunks,
                    });
                    cx.notify();
                });
                block = block.child(
                    Button::new(SharedString::from(format!("hunks-apply-{sid}-{file_idx}")))
                        .label(format!("Apply {n} hunk(s) → main"))
                        .xsmall()
                        .compact()
                        .on_click(apply),
                );
            }
        }
        // Notes attached to this file.
        // Refactor: delete by value (not captured index) so list mutations
        // can't mis-target a note.
        for note in self
            .diff_notes
            .iter()
            .filter(|n| n.session_id == sid && n.file == name)
            .cloned()
            .collect::<Vec<_>>()
        {
            let target = note.clone();
            let target_for_drop = target.clone();
            let drop = cx.listener(move |app, _: &ClickEvent, _, cx| {
                app.diff_notes.retain(|n| n != &target_for_drop);
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
                        Button::new(SharedString::from(format!(
                            "note-del-{}-{}-{}",
                            target.session_id, target.file, target.line
                        )))
                        .label("×")
                        .xsmall()
                        .compact()
                        .on_click(drop),
                    ),
            );
        }
        block
    }

    pub(crate) fn render_diff(&self, cx: &mut Context<Self>) -> AnyElement {
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
