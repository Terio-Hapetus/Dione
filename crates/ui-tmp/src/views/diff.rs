use base::{Command, DiffNote, parse_patch_lines, split_files, worktree::split_hunks};
use gpui::*;
use gpui_component::{ActiveTheme as _, Sizable as _, button::Button, label::Label};

use super::theme::{bad_color, empty_state, muted_for, ok_color, truncate, warn_color};
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

impl FileDiffRow {
    fn git_block(file: String, patch: String) -> Self {
        Self {
            file: Some(file),
            additions: None,
            deletions: None,
            patch: Some(patch),
        }
    }
}

/// Rows for one diff value (pure: unit-tested). Legacy wire shape is a
/// `FileDiffRow` array; git-shape (`GitDiff::to_json`) carries a combined
/// `raw` patch — split per file so annotate + cherry-pick + viewer act
/// on single files. Anything else renders as no rows.
pub(crate) fn file_rows(value: &serde_json::Value) -> Vec<FileDiffRow> {
    if let Ok(rows) = serde_json::from_value::<Vec<FileDiffRow>>(value.clone())
        && !rows.is_empty()
    {
        return rows;
    }
    let raw = value.get("raw").and_then(|r| r.as_str()).unwrap_or("");
    if raw.trim().is_empty() {
        return Vec::new();
    }
    let sections = split_files(raw);
    if sections.is_empty() {
        // No `diff --git` headers (bare hunks): one block as before.
        return vec![FileDiffRow::git_block(
            "(working tree)".to_string(),
            raw.to_string(),
        )];
    }
    sections
        .into_iter()
        .map(|(file, section)| {
            FileDiffRow::git_block(
                file.unwrap_or_else(|| "(working tree)".to_string()),
                section,
            )
        })
        .collect()
}

impl AdeApp {
    pub(crate) fn file_diff_block(
        &self,
        sid: &str,
        file_idx: usize,
        d: &FileDiffRow,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let dark = cx.theme().is_dark();
        let name = d.file.clone().unwrap_or_else(|| "(unknown)".into());
        let title = format!(
            "{name}  +{} −{}",
            d.additions.unwrap_or(0.),
            d.deletions.unwrap_or(0.)
        );
        // Single-file blocks open in the viewer tab (M8e). Multi-file
        // git blocks (`"a.rs, b.rs"`) stay static — open files one by one.
        let openable = name != "(unknown)" && name != "(working tree)" && !name.contains(", ");
        let mut block = div().flex().flex_col().gap_0p5().child({
            let label = Label::new(title);
            if openable {
                let open_sid = sid.to_string();
                let open_name = name.clone();
                let open = cx.listener(move |app, _: &ClickEvent, _, cx| {
                    let scope = app.store.scope_of(&open_sid).to_string();
                    app.open_file = Some(crate::views::file::open_path(
                        &app.store, &scope, &open_name,
                    ));
                    app.right_tab = crate::app::RightTab::File;
                    cx.notify();
                });
                div()
                    .id(SharedString::from(format!("file-open-{sid}-{file_idx}")))
                    .cursor_pointer()
                    .on_click(open)
                    .child(label)
                    .into_any_element()
            } else {
                div().child(label).into_any_element()
            }
        });
        if let Some(patch) = d.patch.clone() {
            let mut lines = div().flex().flex_col();
            // Track which @@ hunk each rendered row belongs to so @@ rows
            // get a cherry-pick toggle (M8a). Preamble rows get None.
            let mut hunk_idx: Option<usize> = None;
            let parsed = parse_patch_lines(&patch);
            let total = parsed.len();
            for (li, pl) in parsed.iter().take(400).enumerate() {
                let color = if pl.text.starts_with('+') && !pl.text.starts_with("+++") {
                    ok_color()
                } else if pl.text.starts_with('-') && !pl.text.starts_with("---") {
                    bad_color()
                } else if pl.text.starts_with("@@") {
                    rgba(0xb18cf0ff)
                } else {
                    muted_for(dark)
                };
                if pl.text.starts_with("@@") {
                    hunk_idx = Some(hunk_idx.map_or(0, |i| i + 1));
                }
                // Gutter: new-file line number when the parser mapped one
                // (UX5) — makes notes like "L12" findable in the patch.
                let row = match pl.line {
                    Some(n) => div().flex().gap_2().child(
                        div().w(px(36.)).flex_none().child(
                            Label::new(format!("{n}"))
                                .text_size(px(11.))
                                .text_color(muted_for(dark)),
                        ),
                    ),
                    None => div().flex().gap_2(),
                }
                .child(Label::new(pl.text.clone()).text_color(color));
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
                            // A line click starts annotating: any pending
                            // reply yields to the new target.
                            app.reply_target = None;
                            // Two-click range select (M8c): first click
                            // anchors, second click in the same file ranges.
                            let (anchor, range) =
                                base::resolve_range(app.annotate_anchor.clone(), target.clone());
                            app.annotate_anchor = anchor;
                            app.annotate_target = range.map(|(a, b)| {
                                (target.0.clone(), target.1.clone(), a, (b > a).then_some(b))
                            });
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
            if total > 400 {
                lines = lines.child(
                    Label::new(format!(
                        "… +{} more lines — open the file in the File tab",
                        total - 400
                    ))
                    .text_size(px(11.))
                    .text_color(muted_for(dark)),
                );
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
                let apply_file = name.clone();
                let apply_patch = patch.clone();
                let apply = cx.listener(move |app, _: &ClickEvent, _, cx| {
                    let all = split_hunks(&apply_patch);
                    let hunks = picked
                        .iter()
                        .filter_map(|i| all.get(*i).cloned())
                        .collect::<Vec<_>>();
                    // Stale picks (diff refreshed shorter) send nothing
                    // rather than a misleading "applied 0". Picks are kept
                    // on failure so the user can retry after resolving.
                    if hunks.len() == picked.len() && !hunks.is_empty() {
                        app.rt.send(Command::ApplyHunks {
                            file: apply_file.clone(),
                            hunks,
                        });
                    }
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
            let target_for_reply = target.clone();
            // Button ids include the range end so L12 and L12-18 never
            // share a GPUI id.
            let id_end = target
                .end_line
                .filter(|e| *e > target.line)
                .map(|e| format!("-{e}"))
                .unwrap_or_default();
            let reply = cx.listener(move |app, _: &ClickEvent, _, cx| {
                app.reply_target = Some(target_for_reply.clone());
                app.annotate_target = None;
                app.annotate_anchor = None;
                cx.notify();
            });
            let mut note_block = div().flex().flex_col().gap_0p5().child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl_2()
                    .child(
                        Label::new(format!(
                            "✎ L{}: {}",
                            match note.end_line {
                                Some(e) if e > note.line => format!("{}-{}", note.line, e),
                                _ => format!("{}", note.line),
                            },
                            truncate(&note.text, 120)
                        ))
                        .text_size(px(11.))
                        .text_color(warn_color()),
                    )
                    .child(
                        Button::new(SharedString::from(format!(
                            "note-reply-{}-{}-{}{}",
                            target.session_id, target.file, target.line, id_end
                        )))
                        .label("Reply")
                        .xsmall()
                        .compact()
                        .on_click(reply),
                    )
                    .child(
                        Button::new(SharedString::from(format!(
                            "note-del-{}-{}-{}{}",
                            target.session_id, target.file, target.line, id_end
                        )))
                        .label("×")
                        .xsmall()
                        .compact()
                        .on_click(drop),
                    ),
            );
            for r in &note.replies {
                note_block = note_block.child(
                    div().pl_6().child(
                        Label::new(format!("↳ {}", truncate(r, 120)))
                            .text_size(px(11.))
                            .text_color(muted_for(dark)),
                    ),
                );
            }
            block = block.child(note_block);
        }
        block
    }

    pub(crate) fn render_diff(&self, cx: &mut Context<Self>) -> AnyElement {
        let dark = cx.theme().is_dark();
        if self.store.diffs.is_empty() {
            return empty_state("Δ", "No diffs yet", "press ↻ all to fetch", dark);
        }
        // Group sessions with diffs by scope, longest-waiting first (M8b
        // fair queue: the main scope competes on equal terms).
        let scopes: Vec<String> = self.store.sort_review_scopes(
            self.store
                .diffs
                .keys()
                .map(|sid| self.store.scope_of(sid).to_string())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
        );

        let mut col = div().flex().flex_col().gap_2();
        for scope in scopes {
            let header = if scope.is_empty() {
                "main".to_string()
            } else {
                format!("⑂ {scope}")
            };
            // Scope header: title row + its own actions row (UX5) so
            // Merge/Hand off/Branch never squeeze the session rows.
            let mut section = div().flex().flex_col().gap_1().child(
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
                let handoff_slug = scope.clone();
                let handoff = cx.listener(move |app, _: &ClickEvent, _, _| {
                    app.rt.send(Command::HandOffToLocal {
                        slug: handoff_slug.clone(),
                    });
                });
                // Branch name from the composer, else `local/<slug>`
                // (same pattern as sidebar `+ wt`).
                let branch_slug = scope.clone();
                let branch = cx.listener(move |app, _: &ClickEvent, window, cx| {
                    let typed = app.input.read(cx).value().to_string();
                    let name = if typed.trim().is_empty() {
                        format!("local/{branch_slug}")
                    } else {
                        typed
                    };
                    app.rt.send(Command::CreateBranchHere {
                        slug: branch_slug.clone(),
                        name,
                    });
                    app.input.update(cx, |st, cx| st.set_value("", window, cx));
                });
                // Merge winner is the primary action; Hand off / Branch
                // here are secondary (outline) — same hierarchy as the
                // permission buttons (UX7 will finish that side).
                section = section.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            Button::new(SharedString::from(format!("merge-{scope}")))
                                .label("Merge winner")
                                .xsmall()
                                .compact()
                                .on_click(merge),
                        )
                        .child(
                            Button::new(SharedString::from(format!("handoff-{scope}")))
                                .label("Hand off")
                                .xsmall()
                                .compact()
                                .outline()
                                .on_click(handoff),
                        )
                        .child(
                            Button::new(SharedString::from(format!("branch-{scope}")))
                                .label("Branch here")
                                .xsmall()
                                .compact()
                                .outline()
                                .on_click(branch),
                        ),
                );
            }
            // Sessions in review order: needs-you first, then longest
            // waiting (M8b). Replaces the old lexicographic sid sort.
            let sids = self.store.sort_review_sids(
                self.store
                    .diffs
                    .keys()
                    .filter(|sid| self.store.scope_of(sid) == scope)
                    .cloned()
                    .collect(),
            );
            for sid in sids {
                let notes: Vec<DiffNote> = self
                    .diff_notes
                    .iter()
                    .filter(|n| n.session_id == sid)
                    .cloned()
                    .collect();
                let short: String = sid.chars().take(8).collect();
                let session_label = if notes.is_empty() {
                    format!("session {short}")
                } else {
                    format!("session {short} · {} note(s)", notes.len())
                };
                let mut head = div().flex().items_center().justify_between().child(
                    Label::new(session_label)
                        .text_size(px(11.))
                        .text_color(muted_for(dark)),
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
                let rows = file_rows(value);
                section = section.child(
                    Label::new(format!("{} file(s) — click a line to annotate", rows.len()))
                        .text_size(px(11.))
                        .text_color(muted_for(dark)),
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

#[cfg(test)]
mod tests {
    // NOTE: same pitfall as vm_badge — no `use super::*`; the file's
    // `use gpui::*` glob would shadow builtin `#[test]`.
    use serde_json::json;

    use base::split_files;

    use crate::views::diff::file_rows;

    #[test]
    fn legacy_array_passes_through() {
        let v = json!([
            {"file": "a.rs", "additions": 1.0, "deletions": 0.0, "patch": "@@ -1 +1 @@\n+x"},
            {"file": "b.rs", "patch": "@@ -2 +2 @@\n+y"},
        ]);
        let rows = file_rows(&v);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].file.as_deref(), Some("a.rs"));
        assert!(rows[0].patch.as_deref().is_some_and(|p| p.contains("+x")));
        assert_eq!(rows[1].additions, None);
    }

    #[test]
    fn git_shape_splits_one_block_per_file() {
        let v = serde_json::json!({"source": "git", "files": ["a.rs", "b.rs"], "raw": "diff --git a/a.rs b/a.rs\n@@ -1 +1 @@\n-x\n+y\ndiff --git a/b.rs b/b.rs\n@@ -2 +2 @@\n-p\n+q"});
        let rows = file_rows(&v);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].file.as_deref(), Some("a.rs"));
        assert!(
            rows[0]
                .patch
                .as_deref()
                .is_some_and(|p| p.contains("+y") && !p.contains("+q"))
        );
        assert_eq!(rows[1].file.as_deref(), Some("b.rs"));
        assert!(rows[1].patch.as_deref().is_some_and(|p| p.contains("+q")));
        // Bare hunks without headers stay one working-tree block.
        let v2 = serde_json::json!({"source": "git", "files": [], "raw": "@@ -1 +1 @@\n+z"});
        let rows2 = file_rows(&v2);
        assert_eq!(rows2.len(), 1);
        assert_eq!(rows2[0].file.as_deref(), Some("(working tree)"));
    }

    #[test]
    fn split_files_cuts_at_git_headers() {
        let raw = "diff --git a/a.rs b/a.rs\n@@ -1 +1 @@\n-x\n+y\ndiff --git a/b.rs b/b.rs\nBinary files a/b.rs and b/b.rs differ\n";
        let parts = split_files(raw);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].0.as_deref(), Some("a.rs"));
        assert!(parts[0].1.contains("@@"));
        assert_eq!(parts[1].0.as_deref(), Some("b.rs"));
        assert!(parts[1].1.contains("Binary"));
        assert!(split_files("@@ -1 +1 @@\n+z").is_empty());
        assert!(split_files("").is_empty());
    }

    #[test]
    fn garbage_renders_no_rows() {
        assert!(file_rows(&json!([])).is_empty());
        assert!(file_rows(&json!({"source": "git", "files": [], "raw": "  \n"})).is_empty());
        assert!(file_rows(&json!({"nope": 1})).is_empty());
    }
}
