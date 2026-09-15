//! UX7 permission gate: same blocking overlay (the agent cannot proceed
//! without a decision — no defer path exists in `Command`), but with a
//! clear action hierarchy + queue position + full command text.
//!
//! - `Allow once` is the primary (default-styled) action.
//! - `Always allow` is secondary (outline).
//! - `✕ Reject` is destructive-separated at the row end (outline) —
//!   no longer three equal-weight buttons.
//! - Header shows `1 of N` when permissions queue up (old code silently
//!   showed only the first).

use ade_core::{Command, PermissionResponse};
use gpui::*;
use gpui_component::{ActiveTheme as _, Sizable as _, button::Button, label::Label};

use super::theme::{TEXT_META, TEXT_SECONDARY, card_bg, muted_for, warn_color};
use crate::app::AdeApp;

impl AdeApp {
    pub(crate) fn render_permission_overlay(
        &self,
        p: ade_core::PendingPermission,
        queue: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let dark = cx.theme().is_dark();
        let once_pid = p.permission_id.clone();
        let always_pid = p.permission_id.clone();
        let reject_pid = p.permission_id.clone();
        let once = cx.listener(move |app, _: &ClickEvent, _, _| {
            app.rt.send(Command::PermissionReply {
                permission_id: once_pid.clone(),
                response: PermissionResponse::Once,
            });
        });
        let always = cx.listener(move |app, _: &ClickEvent, _, _| {
            app.rt.send(Command::PermissionReply {
                permission_id: always_pid.clone(),
                response: PermissionResponse::Always,
            });
        });
        let reject = cx.listener(move |app, _: &ClickEvent, _, _| {
            app.rt.send(Command::PermissionReply {
                permission_id: reject_pid.clone(),
                response: PermissionResponse::Reject,
            });
        });

        let command = p
            .metadata
            .get("command")
            .and_then(|c| c.as_str())
            .map(str::to_string);
        let title = if queue > 1 {
            format!("🔒 Permission required (1 of {queue})")
        } else {
            "🔒 Permission required".to_string()
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(120.))
            .bg(rgba(0x00000099))
            .child(
                div()
                    .w(px(460.))
                    .rounded_lg()
                    .border_1()
                    .border_color(warn_color())
                    .bg(card_bg(dark))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(Label::new(title).text_size(px(15.)))
                    .child(Label::new(p.kind.clone()).text_color(warn_color()))
                    .children(command.map(|c| {
                        Label::new(c)
                            .text_size(px(TEXT_SECONDARY))
                            .text_color(muted_for(dark))
                    }))
                    .children((!p.patterns.is_empty()).then(|| {
                        Label::new(p.patterns.join(", "))
                            .text_size(px(TEXT_SECONDARY))
                            .text_color(muted_for(dark))
                    }))
                    .child(
                        Label::new("The agent waits until you decide.")
                            .text_size(px(TEXT_META))
                            .text_color(muted_for(dark)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .pt_1()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        Button::new("perm-once")
                                            .label("Allow once")
                                            .small()
                                            .on_click(once),
                                    )
                                    .child(
                                        Button::new("perm-always")
                                            .label("Always allow")
                                            .small()
                                            .outline()
                                            .on_click(always),
                                    ),
                            )
                            .child(
                                Button::new("perm-reject")
                                    .label("✕ Reject")
                                    .small()
                                    .outline()
                                    .on_click(reject),
                            ),
                    ),
            )
    }
}
