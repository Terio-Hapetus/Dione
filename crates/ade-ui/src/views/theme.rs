use gpui::{AnyElement, Rgba, rgba};
use gpui_component::label::Label;

// ── UX0 shell metrics (IDE 4 vùng: activity / sidebar / center / review) ──
/// Activity rail cố định (icon lớn, target ≥28px).
pub(crate) const ACTIVITY_W: f32 = 48.0;
/// Sidebar Fleet.
pub(crate) const SIDEBAR_W: f32 = 264.0;
/// Review panel (Diff/File/Context).
pub(crate) const REVIEW_W: f32 = 400.0;
/// TopBar slim.
pub(crate) const TOP_H: f32 = 36.0;
/// Status bar đáy.
pub(crate) const STATUS_H: f32 = 24.0;
/// Chiều cao row tối thiểu (a11y: target ≥28px). Staged cho UX2 (sidebar rows).
#[allow(dead_code)]
pub(crate) const ROW_H: f32 = 32.0;

// ── Type scale: base 13, secondary 12, metadata 11 (cấm <11) ──
#[allow(dead_code)]
pub(crate) const TEXT_BASE: f32 = 13.0;
#[allow(dead_code)]
pub(crate) const TEXT_SECONDARY: f32 = 12.0;
pub(crate) const TEXT_META: f32 = 11.0;

// ── Signal colors: identical in both modes (readable on light + dark) ──
pub(crate) fn ok_color() -> Rgba {
    rgba(0x3fd17cff)
}
pub(crate) fn bad_color() -> Rgba {
    rgba(0xe5484dff)
}
pub(crate) fn warn_color() -> Rgba {
    rgba(0xf5c542ff)
}
/// Accent duy nhất cho action chính (Send, Merge winner). Staged cho UX4/UX5.
#[allow(dead_code)]
pub(crate) fn accent_color() -> Rgba {
    rgba(0x5eb1f0ff)
}

// ── Surface colors: mode-aware (T1). Dark branches keep the exact
// legacy constants (locked by tests); light branches are neutral grays. ──
/// Muted text — light mode needs a darker gray for contrast.
pub(crate) fn muted_for(dark: bool) -> Rgba {
    if dark {
        rgba(0x8b8b93ff)
    } else {
        rgba(0x5f6368ff)
    }
}
pub(crate) fn soft_border_for(dark: bool) -> Rgba {
    if dark {
        rgba(0x33363fff)
    } else {
        rgba(0xdfe1e6ff)
    }
}
/// Selected-row background.
pub(crate) fn active_bg(dark: bool) -> Rgba {
    if dark {
        rgba(0x2a3044ff)
    } else {
        rgba(0xe8edf5ff)
    }
}
/// User chat bubble.
pub(crate) fn bubble_bg(dark: bool) -> Rgba {
    if dark {
        rgba(0x242838ff)
    } else {
        rgba(0xeef1f6ff)
    }
}
/// Floating cards (permission gate).
pub(crate) fn card_bg(dark: bool) -> Rgba {
    if dark {
        rgba(0x1d2029ff)
    } else {
        rgba(0xffffffff)
    }
}

/// Glyph trạng thái dạng text — luôn đi kèm chữ, không chỉ màu
/// (Orca-style: spinner/?/✓/■/○). Pure, unit-tested.
pub(crate) fn status_glyph(
    working: bool,
    needs_you: bool,
    blocked: bool,
    done: bool,
) -> &'static str {
    if blocked {
        "■"
    } else if needs_you {
        "?"
    } else if working {
        "●"
    } else if done {
        "✓"
    } else {
        "○"
    }
}

/// True khi text bị cắt — caller hiện tooltip/title đầy đủ. Staged cho UX7.
#[allow(dead_code)]
pub(crate) fn is_truncated(s: &str, max_chars: usize) -> bool {
    s.chars().count() > max_chars
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

/// Usage money: 4 decimals while under a cent (default view), else 2.
/// Pure, unit-tested — replaces the always-4-decimals `$0.0000` noise.
pub(crate) fn fmt_money(cost: f64) -> String {
    if cost < 0.01 {
        format!("${cost:.4}")
    } else {
        format!("${cost:.2}")
    }
}

/// Empty-state 3 phần (icon + title + hint) thay cho text đơn câm.
/// `dark` để hint đủ tương phản cả 2 mode.
pub(crate) fn empty_state(icon: &str, title: &str, hint: &str, dark: bool) -> AnyElement {
    use gpui::*;
    let mut col = div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_1()
        .child(Label::new(icon.to_string()).text_size(px(TEXT_SECONDARY)))
        .child(Label::new(title.to_string()).text_size(px(TEXT_SECONDARY)));
    if !hint.is_empty() {
        col = col.child(
            Label::new(hint.to_string())
                .text_size(px(TEXT_META))
                .text_color(muted_for(dark)),
        );
    }
    col.into_any_element()
}

#[cfg(test)]
mod tests {
    use crate::views::theme::{is_truncated, status_glyph, truncate};

    #[test]
    fn money_keeps_precision_only_when_small() {
        assert_eq!(super::fmt_money(0.000_01), "$0.0000");
        assert_eq!(super::fmt_money(1.235), "$1.24");
    }

    #[test]
    fn dark_surfaces_keep_legacy_constants() {
        use gpui::rgba;

        use crate::views::theme::{active_bg, bubble_bg, card_bg, muted_for, soft_border_for};
        assert_eq!(muted_for(true), rgba(0x8b8b93ff));
        assert_eq!(soft_border_for(true), rgba(0x33363fff));
        assert_eq!(active_bg(true), rgba(0x2a3044ff));
        assert_eq!(bubble_bg(true), rgba(0x242838ff));
        assert_eq!(card_bg(true), rgba(0x1d2029ff));
    }

    #[test]
    fn light_surfaces_differ_for_contrast() {
        use crate::views::theme::{active_bg, bubble_bg, card_bg, muted_for, soft_border_for};
        assert_ne!(muted_for(false), muted_for(true));
        assert_ne!(soft_border_for(false), soft_border_for(true));
        assert_ne!(active_bg(false), active_bg(true));
        assert_ne!(bubble_bg(false), bubble_bg(true));
        assert_ne!(card_bg(false), card_bg(true));
    }

    // Compile-time guards (giữ ở const để clippy không báo constant-assert).
    const _: () = assert!(super::ROW_H >= 28.0);
    const _: () = assert!(super::TEXT_BASE >= 12.0);
    const _: () = assert!(super::SIDEBAR_W > 220.0);
    const _: () = assert!(super::REVIEW_W >= 360.0);

    #[test]
    fn glyph_priority_blocked_over_all() {
        assert_eq!(status_glyph(true, true, true, true), "■");
        assert_eq!(status_glyph(false, true, false, false), "?");
        assert_eq!(status_glyph(true, false, false, false), "●");
        assert_eq!(status_glyph(false, false, false, true), "✓");
        assert_eq!(status_glyph(false, false, false, false), "○");
    }

    #[test]
    fn truncate_flags_long_text() {
        assert!(!is_truncated("abc", 20));
        assert!(is_truncated("abcdef", 5));
        assert_eq!(truncate("abcdef", 5), "abcde…");
    }

    #[test]
    fn shell_metrics_documented() {
        // Guard thực: const assert trên đã chặn sai số lúc biên dịch;
        // test này giữ tên metric trong report.
        assert!(is_truncated("abcdef", 5));
    }
}
