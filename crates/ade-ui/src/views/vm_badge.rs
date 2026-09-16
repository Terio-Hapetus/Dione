use ade_workspace::ContainerState;
use gpui::*;
use gpui_component::{Sizable as _, label::Label};

use super::theme::{TEXT_META, bad_color, muted_for, ok_color, truncate, warn_color};
use crate::app::AdeApp;

/// Short text for a container lifecycle state (pure: unit-tested).
pub(crate) fn vm_label(state: &ContainerState) -> &'static str {
    match state {
        ContainerState::Missing => "missing",
        ContainerState::Pulling => "pulling…",
        ContainerState::Running => "running",
        ContainerState::Stopped => "stopped",
        ContainerState::Error(_) => "error",
    }
}

/// Badge dot for a container state. `None` = steady/invisible states.
pub(crate) fn vm_dot(state: &ContainerState) -> Option<Rgba> {
    match state {
        ContainerState::Running => Some(ok_color()),
        ContainerState::Error(_) => Some(bad_color()),
        ContainerState::Missing | ContainerState::Stopped => None,
        ContainerState::Pulling => Some(warn_color()),
    }
}

impl AdeApp {
    /// Persistent banner when the machine cannot do containers.
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
                .child(Label::new("Podman unavailable — Host mode").text_color(warn_color()))
        }))
    }

    /// Fleet-row suffix for a workspace container state, if tracked.
    pub(crate) fn vm_badge(&self, slug: &str, dark: bool) -> Option<AnyElement> {
        let state = self.vm_states.get(slug)?;
        let dot = vm_dot(state)?;
        let label = format!("ctr:{}", vm_label(state));
        // The 264px Fleet row can't fit long endpoints — cap the badge.
        // Full endpoint stays one click away via the SSH tab (W3).
        let label = truncate(&label, 28);
        Some(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    Label::new(label)
                        .text_size(px(TEXT_META))
                        .text_color(muted_for(dark)),
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
    use ade_workspace::ContainerState;

    use crate::views::vm_badge::{vm_dot, vm_label};

    #[test]
    fn labels_cover_all_states() {
        assert_eq!(vm_label(&ContainerState::Missing), "missing");
        assert_eq!(vm_label(&ContainerState::Pulling), "pulling…");
        assert_eq!(vm_label(&ContainerState::Running), "running");
        assert_eq!(vm_label(&ContainerState::Stopped), "stopped");
        assert_eq!(vm_label(&ContainerState::Error("x".into())), "error");
    }

    #[test]
    fn steady_states_have_no_dot() {
        assert!(vm_dot(&ContainerState::Missing).is_none());
        assert!(vm_dot(&ContainerState::Stopped).is_none());
        assert!(vm_dot(&ContainerState::Running).is_some());
        assert!(vm_dot(&ContainerState::Pulling).is_some());
        assert!(vm_dot(&ContainerState::Error("x".into())).is_some());
    }
}
