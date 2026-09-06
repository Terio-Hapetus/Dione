use gpui::{AnyElement, Rgba, rgba};
use gpui_component::label::Label;

pub(crate) fn ok_color() -> Rgba {
    rgba(0x3fd17cff)
}
pub(crate) fn bad_color() -> Rgba {
    rgba(0xe5484dff)
}
pub(crate) fn warn_color() -> Rgba {
    rgba(0xf5c542ff)
}
pub(crate) fn muted_color() -> Rgba {
    rgba(0x8b8b93ff)
}
pub(crate) fn soft_border() -> Rgba {
    rgba(0x33363fff)
}

pub(crate) fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max_chars).collect::<String>())
    }
}

pub(crate) fn fmt_tok(n: f64) -> String {
    if n >= 10_000. {
        format!("{:.1}k", n / 1000.)
    } else {
        format!("{n:.0}")
    }
}

pub(crate) fn v_center(text: &str) -> AnyElement {
    use gpui::*;
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(Label::new(text.to_string()).text_color(muted_color()))
        .into_any_element()
}
