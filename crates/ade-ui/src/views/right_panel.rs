use ade_core::{Command, Store};
use gpui::*;
use gpui_component::{ActiveTheme as _, Sizable as _, button::Button, label::Label};

use super::theme::{REVIEW_W, fmt_tok, muted_color, ok_color, truncate};
use crate::app::{AdeApp, RightTab};

impl AdeApp {
    pub(crate) fn render_right_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = [
            (RightTab::Context, "context"),
            (RightTab::Diff, "diff"),
            (RightTab::File, "file"),
        ];
        let mut header = div().flex().gap_1().px_2().py_1();
        for (t, name) in tabs {
            let set = cx.listener(move |app, _: &ClickEvent, _, _| {
                app.right_tab = t;
            });
            let active = self.right_tab == t;
            let label: SharedString = if active {
                format!("{name} ●").into()
            } else {
                SharedString::from(name)
            };
            header = header.child(
                Button::new(SharedString::from(format!("tab-{name}")))
                    .label(label)
                    .xsmall()
                    .compact()
                    .on_click(set),
            );
        }
        if self.right_tab == RightTab::Diff {
            let fetch_all = cx.listener(move |app, _: &ClickEvent, _, _| {
                app.rt.send(Command::FetchAllDiffs);
            });
            header = header.child(
                Button::new("diff-fetch")
                    .label("↻ all")
                    .xsmall()
                    .compact()
                    .on_click(fetch_all),
            );
        }

        let body: AnyElement = match self.right_tab {
            RightTab::Context => context_view(&self.store, cx),
            RightTab::Diff => self.render_diff(cx),
            RightTab::File => self.render_file(cx),
        };

        div()
            .w(px(REVIEW_W))
            .flex_none()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(cx.theme().border)
            .child(header)
            .child(
                div()
                    .id("right-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .py_1()
                    .child(body),
            )
    }
}

pub(crate) fn context_view(store: &Store, cx: &mut Context<AdeApp>) -> AnyElement {
    use ade_core::context::{SectionKind, compile};
    let view = compile(store);
    let border = cx.theme().border;

    let mut col = div().flex().flex_col().gap_1().child(
        Label::new(format!(
            "est ≈ {} tok · last step {:.0} tok",
            fmt_tok(view.est_total_tokens as f64),
            view.actual_total.unwrap_or(0.)
        ))
        .text_color(ok_color()),
    );

    for s in &view.sections {
        let color = match s.kind {
            SectionKind::System => rgba(0xb18cf0ff),
            SectionKind::User => rgba(0x5eb1f0ff),
            SectionKind::Assistant => ok_color(),
            SectionKind::Reasoning => muted_color(),
            SectionKind::ToolCall => super::theme::warn_color(),
            SectionKind::Other => muted_color(),
        };
        col = col.child(
            div()
                .rounded_sm()
                .border_1()
                .border_color(border)
                .px_2()
                .py_1()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(Label::new(s.label.clone()).text_color(color))
                        .child(
                            Label::new(fmt_tok(s.est_tokens as f64))
                                .text_size(px(10.))
                                .text_color(muted_color()),
                        ),
                )
                .children((!s.detail.is_empty()).then(|| {
                    Label::new(truncate(&s.detail, 260))
                        .text_size(px(11.))
                        .text_color(muted_color())
                })),
        );
    }
    col.into_any_element()
}
