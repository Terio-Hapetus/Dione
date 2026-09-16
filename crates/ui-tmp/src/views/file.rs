//! Read-only file viewer (M8e, text-first).
//!
//! Opens a file from a diff block into the right panel: line numbers +
//! plain text, capped by size and line count. Syntax highlighting plugs
//! in later via `gpui-component`'s `LanguageRegistry::register` — the
//! `lang` tag is already carried for that; until then every language
//! renders as plain text (never through `code_editor`, which would
//! mis-tint unknown languages as JSON).

use std::path::{Path, PathBuf};

use base::Store;
use gpui::*;
use gpui_component::{ActiveTheme as _, Sizable as _, button::Button, label::Label};

use super::theme::{empty_state, muted_for, truncate, warn_color};
use crate::app::DioneApp;
use crate::container_thread::workspace_root;

pub(crate) const MAX_FILE_BYTES: usize = 256 * 1024;
pub(crate) const MAX_FILE_LINES: usize = 2000;

/// An opened file: checkout-relative path plus capped content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenFile {
    pub scope: String,
    pub relpath: String,
    pub content: String,
    pub truncated: bool,
    /// Extension tag (`rs`, `toml`, …) for future highlight grammars.
    pub lang: String,
}

/// Language tag from a relative path (pure: unit-tested).
pub(crate) fn lang_of(relpath: &str) -> &'static str {
    match relpath.rsplit('.').next().unwrap_or("") {
        "rs" => "rust",
        "toml" => "toml",
        "py" => "python",
        "js" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        "json" => "json",
        "md" => "markdown",
        "yaml" | "yml" => "yaml",
        "sh" => "bash",
        "diff" | "patch" => "diff",
        _ => "text",
    }
}

/// Checkout directory for a diff scope (pure over the store): worktree
/// scopes use their recorded path; the main scope (`""`) derives the
/// repo root from any known worktree, else refuses.
pub(crate) fn checkout_for(store: &Store, scope: &str) -> Option<PathBuf> {
    if scope.is_empty() {
        store
            .worktrees
            .values()
            .next()
            .map(|r| workspace_root(&r.path))
    } else {
        store.worktrees.get(scope).map(|r| r.path.clone())
    }
}

/// Read a file for viewing: rejects binaries, caps bytes. Returns the
/// text plus whether it was cut short.
pub(crate) fn load_file(path: &Path) -> Result<(String, bool), String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("cannot read {}: {e:#}", path.display()))?;
    if bytes.contains(&0) {
        return Err(format!("{} looks binary — not shown", path.display()));
    }
    let truncated = bytes.len() > MAX_FILE_BYTES;
    let end = bytes.len().min(MAX_FILE_BYTES);
    Ok((
        String::from_utf8_lossy(&bytes[..end]).into_owned(),
        truncated,
    ))
}

/// Open a checkout-relative path for the viewer. Failures become an
/// inline viewer message (the UI snapshot cannot push store errors).
pub(crate) fn open_path(store: &Store, scope: &str, relpath: &str) -> OpenFile {
    let err_file = |msg: String| OpenFile {
        scope: scope.to_string(),
        relpath: relpath.to_string(),
        content: msg,
        truncated: false,
        lang: "text".into(),
    };
    // Stay inside the checkout: reject absolute paths and `..`.
    let rel = Path::new(relpath);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        return err_file(format!(
            "refusing to open {relpath:?}: outside the checkout"
        ));
    }
    let Some(root) = checkout_for(store, scope) else {
        return err_file("no checkout known for this scope yet".to_string());
    };
    match load_file(&root.join(rel)) {
        Ok((mut content, mut truncated)) => {
            let lines: Vec<&str> = content.lines().collect();
            if lines.len() > MAX_FILE_LINES {
                content = lines[..MAX_FILE_LINES].join("\n");
                truncated = true;
            }
            OpenFile {
                scope: scope.to_string(),
                relpath: relpath.to_string(),
                content,
                truncated,
                lang: lang_of(relpath).to_string(),
            }
        }
        Err(msg) => err_file(msg),
    }
}

impl DioneApp {
    pub(crate) fn render_file(&self, cx: &mut Context<Self>) -> AnyElement {
        let dark = cx.theme().is_dark();
        let Some(f) = self.open_file.as_ref() else {
            return empty_state("🗎", "No file open", "click a file name in a diff", dark);
        };
        let close = cx.listener(move |app, _: &ClickEvent, _, cx| {
            app.open_file = None;
            cx.notify();
        });
        let mut col = div()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Label::new(format!(
                            "{}{}  ·  {}",
                            if f.scope.is_empty() {
                                "main".to_string()
                            } else {
                                format!("⑂ {}", f.scope)
                            },
                            format!("/{}", f.relpath),
                            f.lang
                        ))
                        .text_size(px(11.))
                        .text_color(warn_color()),
                    )
                    .child(
                        Button::new("file-close")
                            .label("×")
                            .xsmall()
                            .compact()
                            .on_click(close),
                    ),
            )
            .child(
                Label::new(if f.truncated {
                    "(truncated — showing the head)".to_string()
                } else {
                    format!("{} line(s)", f.content.lines().count())
                })
                .text_size(px(10.))
                .text_color(muted_for(dark)),
            );
        for (i, line) in f.content.lines().enumerate() {
            col = col.child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Label::new(format!("{:>4}", i + 1))
                            .text_size(px(11.))
                            .text_color(muted_for(dark)),
                    )
                    .child(Label::new(truncate(line, 240)).text_size(px(11.))),
            );
        }
        col.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // NOTE: same pitfall as vm_badge — no `use super::*`; the file's
    // `use gpui::*` glob would shadow builtin `#[test]`.
    use base::Store;
    use base::worktree::WorktreeRecord;

    use crate::views::file::{MAX_FILE_BYTES, checkout_for, lang_of, load_file, open_path};

    #[test]
    fn lang_tags_cover_common_extensions() {
        assert_eq!(lang_of("src/main.rs"), "rust");
        assert_eq!(lang_of("Cargo.toml"), "toml");
        assert_eq!(lang_of("x.py"), "python");
        assert_eq!(lang_of("notes.md"), "markdown");
        assert_eq!(lang_of("Makefile"), "text");
        assert_eq!(lang_of("noext"), "text");
    }

    #[test]
    fn checkout_resolves_worktree_and_derived_root() {
        let mut store = Store::default();
        assert!(checkout_for(&store, "").is_none());
        assert!(checkout_for(&store, "wt-a").is_none());
        let repo = std::path::Path::new("/repo");
        let record = WorktreeRecord::new(repo, "wt-a").unwrap();
        let wt_path = record.path.clone();
        store.upsert_worktree(record);
        assert_eq!(checkout_for(&store, "wt-a"), Some(wt_path));
        // Main scope derives the repo root from any known worktree.
        assert_eq!(checkout_for(&store, ""), Some(repo.to_path_buf()));
    }

    #[test]
    fn load_file_rejects_binary_and_caps() {
        let dir = std::env::temp_dir().join(format!("dione-file-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bin.dat"), [0u8, 1, 2]).unwrap();
        assert!(load_file(&dir.join("bin.dat")).is_err());
        let big = "x".repeat(MAX_FILE_BYTES + 10);
        std::fs::write(dir.join("big.txt"), &big).unwrap();
        let (text, truncated) = load_file(&dir.join("big.txt")).unwrap();
        assert!(truncated);
        assert_eq!(text.len(), MAX_FILE_BYTES);
        assert!(load_file(&dir.join("missing.txt")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_path_stays_inside_checkout() {
        let store = Store::default();
        let f = open_path(&store, "", "../evil.txt");
        assert!(f.content.contains("outside the checkout"));
        let g = open_path(&store, "", "/abs.txt");
        assert!(g.content.contains("outside the checkout"));
        let h = open_path(&store, "wt-unknown", "a.rs");
        assert!(h.content.contains("no checkout known"));
    }
}
