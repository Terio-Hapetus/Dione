use ade_core::ConnState;
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, button::Button, label::Label,
};

use super::theme::{bad_color, muted_color, ok_color, warn_color};
use crate::app::AdeApp;

impl AdeApp {
    pub(crate) fn render_top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (dot, status_text) = match &self.store.conn {
            ConnState::Connected => (ok_color(), "connected"),
            ConnState::Connecting => (warn_color(), "connecting…"),
            ConnState::Disconnected => (bad_color(), "disconnected"),
        };
        let t = &self.store.totals;
        let border = cx.theme().border;

        let models = self.flat_models();
        let current = self.model_label(&models);
        let n = models.len();
        let next = cx.listener(move |this, _: &ClickEvent, _, _| {
            if n > 0 {
                let ix = this.model_ix.map(|i| (i + 1) % n).unwrap_or(0);
                this.pick_model(ix);
            }
        });
        let prev = cx.listener(move |this, _: &ClickEvent, _, _| {
            if n > 0 {
                this.pick_model(this.model_ix.map(|i| (i + n - 1) % n).unwrap_or(n - 1));
            }
        });

        div()
            .h(px(38.))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(border)
            .child(Icon::new(IconName::CircleCheck).small().text_color(dot))
            .child(Label::new(status_text).text_color(muted_color()))
            .child(div().w(px(1.)).h(px(16.)).bg(border))
            .child(
                Button::new("model-prev")
                    .label("<")
                    .xsmall()
                    .compact()
                    .on_click(prev),
            )
            .child(Label::new(current))
            .child(
                Button::new("model-next")
                    .label(">")
                    .xsmall()
                    .compact()
                    .on_click(next),
            )
            .child(div().flex_1())
            .child(
                Label::new(format!(
                    "ctx≈{:.1}k tok · ${:.4}",
                    t.total_context() / 1000.0,
                    t.cost
                ))
                .text_color(muted_color()),
            )
    }
}
