mod app;
mod container_thread;
mod views;

use gpui::{
    AnyView, AppContext as _, Application, Bounds, WindowBounds, WindowDecorations, WindowOptions,
    px, size,
};

fn main() {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("[dione panic] {info}");
    }));

    let config = base::AppConfig::load();
    let theme_pref = config.theme.clone();
    let rt = base::runtime::spawn(config);

    Application::new()
        .with_assets(gpui_component_assets::Assets)
        .run(move |cx| {
            gpui_component::init(cx);

            // Theme resolve (T2): `DIONE_THEME` env > pinned config >
            // system appearance. Applied before the window opens so the
            // first frame already uses the right palette.
            let mode = dione_ui_theme(theme_pref.as_deref(), cx);
            gpui_component::Theme::change(mode, None, cx);

            let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
            // Keep the WM/app-switcher title: transparent options blank it.
            let mut titlebar = gpui_component::TitleBar::title_bar_options();
            titlebar.title = Some("Dione — Agentic IDE".into());
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // Custom client-side titlebar (drag + min/max/close) drawn
                // by `views::shell` when the compositor negotiates
                // `Decorations::Client` (typical Wayland). Server-decorated
                // sessions (X11 + WM) keep the native bar instead.
                titlebar: Some(titlebar),
                // Request client-side decorations explicitly (B2): leaving
                // this None makes GPUI request Server, which strands
                // compositors that neither flip to Client nor draw SSD —
                // a chromeless window. Client request makes our TitleBar
                // deterministic; SSD-forcing compositors (KDE) and
                // compositor-less X11 still fall back to native bars.
                window_decorations: Some(WindowDecorations::Client),
                is_movable: true,
                is_resizable: true,
                is_minimizable: true,
                app_id: Some("dione".to_string()),
                window_min_size: Some(size(px(640.), px(480.))),
                ..Default::default()
            };

            cx.open_window(options, |window, cx| {
                let content = cx.new(|cx| app::DioneApp::new(rt.clone(), window, cx));
                cx.new(|cx| gpui_component::Root::new(AnyView::from(content), window, cx))
            })
            .expect("open Dione window");

            cx.activate(true);
        });
}

/// Resolve the startup theme (pure-ish, unit-tested via `parse_theme`).
/// Precedence: `DIONE_THEME` env > pinned `config.toml` > system appearance.
fn dione_ui_theme(pinned: Option<&str>, cx: &gpui::App) -> gpui_component::ThemeMode {
    if let Some(mode) = std::env::var("DIONE_THEME")
        .ok()
        .as_deref()
        .and_then(parse_theme)
    {
        return mode;
    }
    if let Some(mode) = pinned.and_then(parse_theme) {
        return mode;
    }
    cx.window_appearance().into()
}

fn parse_theme(s: &str) -> Option<gpui_component::ThemeMode> {
    match s.to_lowercase().as_str() {
        "light" => Some(gpui_component::ThemeMode::Light),
        "dark" => Some(gpui_component::ThemeMode::Dark),
        _ => None,
    }
}
