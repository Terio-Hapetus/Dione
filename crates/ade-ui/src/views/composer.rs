use ade_core::Command;
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Disableable as _, Sizable as _, button::Button, input::Input, label::Label,
};

use super::theme::warn_color;
use crate::app::AdeApp;

impl AdeApp {
    pub(crate) fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.store.is_busy();
        let annotating = self.annotate_target.clone();
        let anchoring = self.annotate_anchor.clone();
        let replying = self.reply_target.clone();

        let send = cx.listener(|this, _: &ClickEvent, window, cx| this.send_prompt(window, cx));
        let annotate =
            cx.listener(|this, _: &ClickEvent, window, cx| this.submit_annotate(window, cx));
        let fanout = cx.listener(|this, _: &ClickEvent, window, cx| this.send_fan_out(window, cx));
        let abort = cx.listener(|this, _: &ClickEvent, _, _| this.rt.send(Command::Abort));

        div()
            .flex_none()
            .flex()
            .flex_col()
            .children(annotating.clone().map(|(sid, file, line, end)| {
                let short: String = sid.chars().take(8).collect();
                let loc = match end {
                    Some(e) => format!("{line}-{e}"),
                    None => format!("{line}"),
                };
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_1()
                    .border_t_1()
                    .border_color(warn_color())
                    .child(
                        Label::new(format!("✎ note on {file}:{loc} ({short}) — type + Enter"))
                            .text_size(px(11.))
                            .text_color(warn_color()),
                    )
            }))
            .children(anchoring.clone().map(|(_, file, line)| {
                let clear = cx.listener(move |app, _: &ClickEvent, _, cx| {
                    app.annotate_anchor = None;
                    app.annotate_target = None;
                    cx.notify();
                });
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_1()
                    .border_t_1()
                    .border_color(warn_color())
                    .child(
                        Label::new(format!(
                            "✎ anchor {file}:{line} — click another line for a range, same line for single"
                        ))
                        .text_size(px(11.))
                        .text_color(warn_color()),
                    )
                    .child(
                        Button::new("anchor-clear")
                            .label("×")
                            .xsmall()
                            .compact()
                            .on_click(clear),
                    )
            }))
            .children(replying.clone().map(|n| {
                let loc = match n.end_line {
                    Some(e) if e > n.line => format!("{}-{}", n.line, e),
                    _ => format!("{}", n.line),
                };
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_1()
                    .border_t_1()
                    .border_color(warn_color())
                    .child(
                        Label::new(format!("↳ reply on {}:{loc} — type + Enter", n.file))
                            .text_size(px(11.))
                            .text_color(warn_color()),
                    )
            }))
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
                    } else if annotating.is_some() || replying.is_some() {
                        vec![
                            Button::new("annotate-send")
                                .label("Annotate ⏎")
                                .small()
                                .on_click(annotate)
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
                                .label("⇉ all")
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
