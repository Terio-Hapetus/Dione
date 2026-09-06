use ade_core::{Command, PermissionResponse};
use gpui::*;
use gpui_component::{Sizable as _, button::Button, label::Label};

use super::theme::{muted_color, warn_color};
use crate::app::AdeApp;

impl AdeApp {
    pub(crate) fn render_permission_overlay(
        &self,
        p: ade_core::PendingPermission,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                    .w(px(420.))
                    .rounded_lg()
                    .border_1()
                    .border_color(warn_color())
                    .bg(rgb(0x1d2029))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(Label::new("🔒 Permission required").text_size(px(15.)))
                    .child(Label::new(p.kind.clone()).text_color(warn_color()))
                    .children(command.map(|c| Label::new(c).text_color(muted_color())))
                    .children((!p.patterns.is_empty()).then(|| {
                        Label::new(super::theme::truncate(&p.patterns.join(", "), 120))
                            .text_size(px(11.))
                            .text_color(muted_color())
                    }))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .pt_1()
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
                                    .on_click(always),
                            )
                            .child(
                                Button::new("perm-reject")
                                    .label("Reject")
                                    .small()
                                    .on_click(reject),
                            ),
                    ),
            )
    }
}
