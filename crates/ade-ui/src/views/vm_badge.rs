use ade_vm::VmState;
use gpui::*;
use gpui_component::{Sizable as _, label::Label};

use super::theme::{bad_color, muted_color, ok_color, warn_color};
use crate::app::AdeApp;

/// Short text for a VM lifecycle state (pure: unit-tested).
pub(crate) fn vm_label(state: &VmState) -> &'static str {
    match state {
        VmState::Missing => "missing",
        VmState::PullingImage => "pulling image…",
        VmState::Booting => "booting…",
        VmState::WaitingSsh => "waiting ssh…",
        VmState::Mounting => "mounting…",
        VmState::Ready => "ready",
        VmState::Running => "running",
        VmState::Stopped => "stopped",
        VmState::Error(_) => "error",
    }
}

/// Badge dot for a VM state. `None` = steady/invisible states.
pub(crate) fn vm_dot(state: &VmState) -> Option<Rgba> {
    match state {
        VmState::Ready | VmState::Running => Some(ok_color()),
        VmState::Error(_) => Some(bad_color()),
        VmState::Missing | VmState::Stopped => None,
        _ => Some(warn_color()),
    }
}

impl AdeApp {
    /// Persistent banner when the machine cannot do MicroVMs (Lab 6).
    /// Host mode keeps working; worktrees are kept for manual retry.
    pub(crate) fn render_vm_banner(&self) -> impl IntoElement {
        div().children((!self.vm_available).then(|| {
            div()
                .flex()
                .gap_2()
                .px_3()
                .py_1()
                .border_b_1()
                .border_color(warn_color())
                .child(Label::new("◌").text_color(warn_color()))
                .child(
                    Label::new("VM unavailable (no /dev/kvm) — Host mode").text_color(warn_color()),
                )
        }))
    }

    /// Fleet-row suffix for a workspace VM state, if tracked.
    pub(crate) fn vm_badge(&self, slug: &str) -> Option<AnyElement> {
        let state = self.vm_states.get(slug)?;
        let dot = vm_dot(state)?;
        Some(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    Label::new(format!("vm:{}", vm_label(state)))
                        .text_size(px(10.))
                        .text_color(muted_color()),
                )
                .child(
                    gpui_component::Icon::new(gpui_component::IconName::CircleCheck)
                        .xsmall()
                        .text_color(dot),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    // NOTE: no `use super::*` here — the file's `use gpui::*` glob
    // imports gpui's `test` macro, which would shadow builtin `#[test]`
    // and recurse forever. Explicit paths keep builtin `#[test]`.
    use ade_vm::VmState;

    use crate::views::vm_badge::{vm_dot, vm_label};

    #[test]
    fn labels_cover_all_states() {
        assert_eq!(vm_label(&VmState::Missing), "missing");
        assert_eq!(vm_label(&VmState::PullingImage), "pulling image…");
        assert_eq!(vm_label(&VmState::Booting), "booting…");
        assert_eq!(vm_label(&VmState::WaitingSsh), "waiting ssh…");
        assert_eq!(vm_label(&VmState::Mounting), "mounting…");
        assert_eq!(vm_label(&VmState::Ready), "ready");
        assert_eq!(vm_label(&VmState::Running), "running");
        assert_eq!(vm_label(&VmState::Stopped), "stopped");
        assert_eq!(vm_label(&VmState::Error("x".into())), "error");
    }

    #[test]
    fn steady_states_have_no_dot() {
        assert!(vm_dot(&VmState::Missing).is_none());
        assert!(vm_dot(&VmState::Stopped).is_none());
        assert!(vm_dot(&VmState::Ready).is_some());
        assert!(vm_dot(&VmState::Booting).is_some());
        assert!(vm_dot(&VmState::Error("x".into())).is_some());
    }
}
