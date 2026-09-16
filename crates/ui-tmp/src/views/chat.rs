use base::{Role, UnifiedMessage};
use gpui::*;
use gpui_component::{ActiveTheme as _, label::Label, text::TextView};

use super::theme::{
    bubble_bg, empty_state, fmt_money, muted_for, ok_color, soft_border_for, truncate,
};
use crate::app::AdeApp;

impl AdeApp {
    /// M3d: agent-agnostic chat. Reads only `transcripts`/`costs` —
    /// no opencode wire types.
    pub(crate) fn render_chat(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let dark = cx.theme().is_dark();
        let Some(sid) = self.store.active_session.clone() else {
            return empty_state(
                "💬",
                "No active session",
                "Create one in the sidebar Fleet view",
                dark,
            );
        };
        let msgs: Vec<UnifiedMessage> = self.store.transcript_for_session(&sid).to_vec();
        if msgs.is_empty() {
            return empty_state(
                "💬",
                "No messages yet",
                "Say something below to start the agent",
                dark,
            );
        }
        let mut rows: Vec<AnyElement> = Vec::new();
        if let Some(cost) = self
            .store
            .session_task
            .get(&sid)
            .and_then(|t| self.store.costs.get(t))
        {
            rows.push(
                Label::new(format!(
                    "{} messages · {}",
                    msgs.len(),
                    fmt_money(cost.cost)
                ))
                .text_size(px(11.))
                .text_color(muted_for(dark))
                .into_any_element(),
            );
        }
        for m in &msgs {
            if m.text.trim().is_empty() {
                continue;
            }
            rows.push(self.unified_row(m, window, cx));
        }
        if self.store.is_busy() {
            rows.push(
                Label::new("▌ agent working…")
                    .text_color(ok_color())
                    .into_any_element(),
            );
        }
        div()
            .id("chat-v2")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_3()
            .py_2()
            .children(rows)
            .into_any_element()
    }

    pub(crate) fn unified_row(
        &self,
        m: &UnifiedMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dark = cx.theme().is_dark();
        match m.role {
            Role::User => div()
                .flex()
                .justify_end()
                .pb_3()
                .pt_1()
                .child(
                    div()
                        .max_w(px(640.))
                        .rounded_md()
                        .px_3()
                        .py_2()
                        .bg(bubble_bg(dark))
                        .child(m.text.clone()),
                )
                .into_any_element(),
            Role::Agent => div()
                .flex()
                .justify_start()
                .pb_2()
                .child(div().max_w(px(640.)).child(TextView::markdown(
                    SharedString::from(format!("chat-{}", m.id)),
                    m.text.clone(),
                    window,
                    cx,
                )))
                .into_any_element(),
            Role::Tool => {
                let title = m
                    .tool
                    .as_ref()
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| "tool".to_string());
                div()
                    .my_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(soft_border_for(dark))
                    .px_2()
                    .py_1()
                    .child(
                        Label::new(format!("🔧 {title}"))
                            .text_size(px(11.))
                            .text_color(ok_color()),
                    )
                    .child(
                        Label::new(truncate(&m.text, 500))
                            .text_size(px(11.))
                            .text_color(muted_for(dark)),
                    )
                    .into_any_element()
            }
        }
    }
}
