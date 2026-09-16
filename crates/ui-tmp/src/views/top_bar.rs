use base::{AppConfig, ConnState};
use gpui::*;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, Theme, ThemeMode, button::Button, label::Label,
};

use super::theme::{TOP_H, bad_color, fmt_money, muted_for, ok_color, truncate, warn_color};
use crate::app::AdeApp;

impl AdeApp {
    pub(crate) fn render_top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = cx.theme().is_dark();
        let (dot, status_text) = match &self.store.conn {
            ConnState::Connected => (ok_color(), "connected"),
            ConnState::Connecting => (warn_color(), "connecting…"),
            ConnState::Disconnected => (bad_color(), "disconnected"),
        };
        let t = &self.store.totals;
        let border = cx.theme().border;

        let models = self.flat_models();
        let current = truncate(&self.model_label(&models), 28);
        let n = models.len();
        // Single cycling button (UX8): the old `<`/`>` pair was tiny and
        // undiscoverable; backward cycle wasn't worth two targets.
        let cycle = cx.listener(move |this, _: &ClickEvent, _, _| {
            if n > 0 {
                let ix = this.model_ix.map(|i| (i + 1) % n).unwrap_or(0);
                this.pick_model(ix);
            }
        });
        let term_label = if self.show_terminal { "Chat" } else { "Term" };
        let toggle_term = cx.listener(|this, _: &ClickEvent, _, _| this.toggle_terminal());
        let open_palette = cx.listener(|this, _: &ClickEvent, _, cx| {
            this.show_palette = true;
            cx.notify();
        });
        // Theme toggle (T2): flips the palette live and pins the choice
        // in `config.toml` (`None` pin = follow system, via palette).
        let theme_label = if dark { "☀" } else { "☾" };
        let toggle_theme = cx.listener(|_, _: &ClickEvent, window, cx| {
            let dark = !cx.theme().is_dark();
            Theme::change(
                if dark {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                },
                Some(window),
                cx,
            );
            AppConfig::save_theme(Some(if dark { "dark" } else { "light" }));
        });

        div()
            .h(px(TOP_H))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(border)
            .child(Icon::new(IconName::CircleCheck).small().text_color(dot))
            .child(Label::new(status_text).text_color(muted_for(dark)))
            .child(div().w(px(1.)).h(px(16.)).bg(border))
            .child(
                Button::new("model-cycle")
                    .label(current)
                    .xsmall()
                    .compact()
                    .on_click(cycle),
            )
            .child(self.render_agent_ticks(dark))
            .child(
                Button::new("term-toggle")
                    .label(term_label)
                    .xsmall()
                    .compact()
                    .on_click(toggle_term),
            )
            .child(
                Button::new("palette-open")
                    .label("⌘K")
                    .xsmall()
                    .compact()
                    .on_click(open_palette),
            )
            .child(
                Button::new("theme-toggle")
                    .label(theme_label)
                    .xsmall()
                    .compact()
                    .on_click(toggle_theme),
            )
            .child(div().flex_1())
            .child(
                Label::new(format!(
                    "ctx≈{:.1}k tok · {}",
                    t.total_context() / 1000.0,
                    fmt_money(t.cost)
                ))
                .text_color(muted_for(dark)),
            )
    }

    /// Agent picker ticks (Lab 4): `name ●` present, `name ○` missing.
    /// Empty registry renders nothing.
    pub(crate) fn render_agent_ticks(&self, dark: bool) -> impl IntoElement {
        let mut row = div().flex().items_center().gap_1();
        for name in &self.agent_names {
            let ok = self.agent_ok.get(name).copied().unwrap_or(false);
            let dot = if ok { ok_color() } else { muted_for(dark) };
            row = row.child(
                Label::new(format!("{} {}", name, if ok { "●" } else { "○" }))
                    .text_size(px(11.))
                    .text_color(dot),
            );
        }
        row
    }
}

#[cfg(test)]
mod tests {
    // NOTE: same pitfall as vm_badge — no `use super::*`; the file's
    // `use gpui::*` glob would shadow builtin `#[test]`.
    use std::collections::BTreeMap;

    use crate::app::load_agent_statuses;

    fn write_agents(body: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("ade-picker-{n}.toml"));
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn missing_registry_yields_empty_ticks() {
        let (names, ok): (Vec<String>, BTreeMap<String, bool>) = load_agent_statuses(None);
        assert!(names.is_empty());
        assert!(ok.is_empty());
        let p = std::env::temp_dir().join("ade-picker-does-not-exist.toml");
        assert!(load_agent_statuses(Some(&p)).0.is_empty());
    }

    #[test]
    fn ticks_reflect_probe_results() {
        let p = write_agents(
            "[agents.good]\nbin = \"sh\"\n[agents.bad]\nbin = \"ade-no-such-bin-xyz\"\n",
        );
        let (names, ok) = load_agent_statuses(Some(&p));
        assert_eq!(names, vec!["bad".to_string(), "good".to_string()]);
        assert!(ok["good"]);
        assert!(!ok["bad"]);
    }
}
