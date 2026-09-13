//! UX4 single-mode composer: the input row is *only* for chat.
//!
//! Annotating / replying moved to a dedicated context bar above the
//! input with its own [Attach] + [x] actions — the main Send button
//! never swaps meaning anymore (old 5-mode overload wiped a class of
//! mis-sent prompts). Enter routing in `app.rs` is unchanged.

use ade_core::Command;
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Disableable as _, Sizable as _, button::Button, input::Input, label::Label,
};

use super::theme::{TEXT_META, warn_color};
use crate::app::AdeApp;

impl AdeApp {
    /// Context bar for the pending review note / reply / range anchor.
    /// `None` when the composer is in plain chat mode.
    pub(crate) fn render_note_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let annotating = self.annotate_target.clone();
        let anchoring = self.annotate_anchor.clone();
        let replying = self.reply_target.clone();
        if annotating.is_none() && anchoring.is_none() && replying.is_none() {
            return None;
        }
        let text = if let Some(n) = replying {
            let loc = match n.end_line {
                Some(e) if e > n.line => format!("{}-{}", n.line, e),
                _ => format!("{}", n.line),
            };
            format!(
                "↳ reply on {}:{} — type below, Attach to append",
                n.file, loc
            )
        } else if let Some((sid, file, line, end)) = annotating {
            let short: String = sid.chars().take(8).collect();
            let loc = match end {
                Some(e) => format!("{line}-{e}"),
                None => format!("{line}"),
            };
            format!("✎ note on {file}:{loc} ({short}) — type below, Attach to add")
        } else if let Some((_, file, line)) = anchoring {
            format!("✎ anchor {file}:{line} — click another line for a range")
        } else {
            return None;
        };
        let attach = cx.listener(|this, _: &ClickEvent, window, cx| {
            this.submit_annotate(window, cx);
        });
        let cancel = cx.listener(|this, _: &ClickEvent, _, cx| {
            this.annotate_target = None;
            this.annotate_anchor = None;
            this.reply_target = None;
            cx.notify();
        });
        Some(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .px_3()
                .py_1()
                .border_t_1()
                .border_color(warn_color())
                .child(
                    Label::new(text)
                        .text_size(px(TEXT_META))
                        .text_color(warn_color()),
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(
                            Button::new("note-attach")
                                .label("Attach ⏎")
                                .xsmall()
                                .on_click(attach),
                        )
                        .child(
                            Button::new("note-cancel")
                                .label("×")
                                .xsmall()
                                .on_click(cancel),
                        ),
                ),
        )
    }

    pub(crate) fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.store.is_busy();

        let send = cx.listener(|this, _: &ClickEvent, window, cx| this.send_prompt(window, cx));
        let fanout = cx.listener(|this, _: &ClickEvent, window, cx| this.send_fan_out(window, cx));
        let abort = cx.listener(|this, _: &ClickEvent, _, _| this.rt.send(Command::Abort));

        div()
            .flex_none()
            .flex()
            .flex_col()
            .children(self.render_note_bar(cx))
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
                    } else {
                        vec![
                            Button::new("send")
                                .label("Send ⏎")
                                .small()
                                .disabled(self.store.active_session.is_none())
                                .on_click(send)
                                .into_any_element(),
                            Button::new("fanout")
                                .label("Send all")
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
