//! M6c2 terminal tab: local pty scrollback viewer + input row.
//!
//! The shell is spawned lazily when the tab opens (Host worktree cwd);
//! the 160ms snapshot loop pumps output without blocking. This is a
//! review-lite viewer, not a full VT emulator: ANSI sequences are
//! stripped for display (search lands in m6c3).

use std::collections::VecDeque;
use std::path::PathBuf;

use ade_workspace::ShellChannel;
use ade_workspace::WorkspaceProvider as _;
use ade_workspace::strip_ansi;
use gpui::*;
use gpui_component::{ActiveTheme as _, Sizable as _, button::Button, input::Input, label::Label};

use super::theme::muted_for;
use crate::app::AdeApp;

/// Scrollback cap: oldest lines drop past this.
pub(crate) const TERM_SCROLLBACK_CAP: usize = 2000;

pub(crate) struct TermState {
    shell: Box<dyn ShellChannel>,
    cwd: PathBuf,
    pub(crate) lines: VecDeque<String>,
    tail: String,
    /// Guest shells are not respawned locally on death (re-toggle).
    guest: bool,
}

impl TermState {
    pub(crate) fn new(shell: Box<dyn ShellChannel>, cwd: PathBuf) -> Self {
        Self {
            shell,
            cwd,
            lines: VecDeque::new(),
            tail: String::new(),
            guest: false,
        }
    }

    pub(crate) fn new_guest(shell: Box<dyn ShellChannel>, cwd: PathBuf) -> Self {
        Self {
            shell,
            cwd,
            lines: VecDeque::new(),
            tail: String::new(),
            guest: true,
        }
    }

    /// Append raw pty bytes: split lines, strip ANSI, cap the buffer.
    /// Returns true when at least one line completed.
    pub(crate) fn push_bytes(&mut self, bytes: &[u8]) -> bool {
        self.tail.push_str(&String::from_utf8_lossy(bytes));
        let mut added = false;
        while let Some(pos) = self.tail.find('\n') {
            let raw: String = self.tail.drain(..=pos).collect();
            let line = strip_ansi(raw.trim_end_matches(['\n', '\r']));
            self.lines.push_back(line);
            added = true;
        }
        while self.lines.len() > TERM_SCROLLBACK_CAP {
            self.lines.pop_front();
        }
        added
    }

    fn ensure_alive(&mut self) {
        if !self.guest && !self.shell.is_alive() {
            let cwd = self.cwd.clone();
            if let Ok(shell) = ade_workspace::HostProvider::new().shell(&cwd) {
                self.shell = shell;
            }
        }
    }
}

impl AdeApp {
    /// Active worktree slug + path, if any.
    fn active_checkout(&self) -> Option<(String, std::path::PathBuf)> {
        self.store.active_worktree.as_ref().and_then(|slug| {
            self.store
                .worktrees
                .get(slug)
                .map(|r| (slug.clone(), r.path.clone()))
        })
    }

    /// Is this worktree's guest Ready/Running?
    fn guest_ready(&self, slug: &str) -> bool {
        self.vm_states
            .get(slug)
            .is_some_and(|s| matches!(s, ade_vm::VmState::Ready | ade_vm::VmState::Running))
    }

    /// Show/hide the terminal tab. Guest Ready → SSH shell via the VM
    /// thread (async reply); otherwise a local Host shell, spawned now.
    pub(crate) fn toggle_terminal(&mut self) {
        if self.show_terminal {
            self.show_terminal = false;
            return;
        }
        if self.term.is_some() {
            self.show_terminal = true;
            return;
        }
        let Some((slug, cwd)) = self.active_checkout() else {
            return;
        };
        if self.guest_ready(&slug) {
            self.pending_shell = Some(self.vm.open_shell(slug, cwd));
            self.term_pending = true;
            self.show_terminal = true;
            return;
        }
        match ade_workspace::HostProvider::new().shell(&cwd) {
            Ok(shell) => {
                self.term = Some(TermState::new(shell, cwd));
                self.show_terminal = true;
            }
            Err(e) => eprintln!("terminal spawn failed: {e:#}"),
        }
    }

    /// Collect an arrived guest shell (polled in the snapshot loop).
    /// Returns true when the view needs a repaint.
    pub(crate) fn poll_pending_shell(&mut self) -> bool {
        let Some(rx) = self.pending_shell.as_ref() else {
            return false;
        };
        let Ok(res) = rx.try_recv() else {
            return false;
        };
        self.pending_shell = None;
        self.term_pending = false;
        match res {
            Ok(shell) => {
                let cwd = self
                    .active_checkout()
                    .map(|(_, p)| p)
                    .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
                self.term = Some(TermState::new_guest(shell, cwd));
            }
            Err(e) => {
                eprintln!("guest shell failed: {e}");
                self.show_terminal = false;
            }
        }
        true
    }

    /// Pump shell output into scrollback. Returns true when the view
    /// needs a repaint.
    pub(crate) fn pump_terminal(&mut self) -> bool {
        let Some(term) = self.term.as_mut() else {
            return false;
        };
        let bytes = term.shell.read_available();
        if bytes.is_empty() {
            return false;
        }
        term.push_bytes(&bytes)
    }

    pub(crate) fn send_terminal_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.term_input.read(cx).value().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(term) = self.term.as_mut() {
            term.ensure_alive();
            let mut line = text.into_bytes();
            line.push(b'\n');
            if term.shell.write_bytes(&line).is_ok() {
                self.term_input
                    .update(cx, |st, cx| st.set_value("", window, cx));
            }
        }
    }

    pub(crate) fn render_terminal(&self, cx: &mut Context<Self>) -> AnyElement {
        let dark = cx.theme().is_dark();
        let Some(term) = self.term.as_ref() else {
            let msg = if self.term_pending {
                "connecting to guest…"
            } else {
                "open a worktree (+ wt) to start a terminal"
            };
            return div()
                .flex_1()
                .items_center()
                .justify_center()
                .child(Label::new(msg).text_color(muted_for(dark)))
                .into_any_element();
        };
        let send = cx.listener(|this, _: &ClickEvent, window, cx| {
            this.send_terminal_input(window, cx);
        });
        let query = self.term_query.read(cx).value().to_string();
        let shown: Vec<String> = filter_lines(term.lines.iter().cloned(), &query);
        let count = if query.is_empty() {
            format!("{}", term.lines.len())
        } else {
            format!("{}/{}", shown.len(), term.lines.len())
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("term-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .py_2()
                    .children(shown)
                    .into_any_element(),
            )
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(div().w(px(140.)).child(Input::new(&self.term_query)))
                    .child(
                        Label::new(count)
                            .text_size(px(11.))
                            .text_color(muted_for(dark)),
                    )
                    .child(div().flex_1().min_w_0().child(Input::new(&self.term_input)))
                    .child(Button::new("term-send").label("⏎").small().on_click(send)),
            )
            .into_any_element()
    }
}

/// Substring filter for scrollback search. Empty query shows everything.
/// Case-insensitive (UX8): terminal output case is rarely what you remember.
pub(crate) fn filter_lines(lines: impl Iterator<Item = String>, query: &str) -> Vec<String> {
    if query.is_empty() {
        return lines.collect();
    }
    let q = query.to_lowercase();
    lines.filter(|l| l.to_lowercase().contains(&q)).collect()
}

#[cfg(test)]
mod tests {
    // NOTE: same pitfall as vm_badge — no `use super::*`; the file's
    // `use gpui::*` glob would shadow builtin `#[test]`.
    use crate::views::terminal::{TERM_SCROLLBACK_CAP, TermState, filter_lines};

    struct FakeShell;

    impl ade_workspace::ShellChannel for FakeShell {
        fn write_bytes(&mut self, _data: &[u8]) -> anyhow::Result<()> {
            Ok(())
        }
        fn read_available(&mut self) -> Vec<u8> {
            Vec::new()
        }
        fn resize(&mut self, _cols: u16, _rows: u16) -> anyhow::Result<()> {
            Ok(())
        }
        fn is_alive(&mut self) -> bool {
            true
        }
        fn kill(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn push_bytes_splits_partials_and_caps() {
        let mut t = TermState::new(Box::new(FakeShell), std::path::PathBuf::from("/tmp"));
        assert!(!t.push_bytes(b"part"));
        assert!(t.lines.is_empty());
        assert!(t.push_bytes(b"ial\nsecond\n"));
        let got: Vec<&str> = t.lines.iter().map(String::as_str).collect();
        assert_eq!(got, vec!["partial", "second"]);
        for i in 0..TERM_SCROLLBACK_CAP + 10 {
            t.push_bytes(format!("l{i}\n").as_bytes());
        }
        assert_eq!(t.lines.len(), TERM_SCROLLBACK_CAP);
        assert_eq!(t.lines[0], "l10");
    }

    #[test]
    fn filter_matches_substring_or_all() {
        let lines = vec!["foo".to_string(), "bar".to_string(), "foobar".to_string()];
        assert_eq!(filter_lines(lines.clone().into_iter(), "").len(), 3);
        assert_eq!(
            filter_lines(lines.clone().into_iter(), "foo"),
            vec!["foo".to_string(), "foobar".to_string()]
        );
        assert!(filter_lines(lines.into_iter(), "zzz").is_empty());
    }

    #[test]
    fn filter_ignores_case() {
        let lines = vec!["Error: boom".to_string(), "ok".to_string()];
        assert_eq!(
            filter_lines(lines.into_iter(), "error"),
            vec!["Error: boom".to_string()]
        );
    }
}
