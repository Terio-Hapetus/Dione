mod app;
mod views;
mod vm_thread;

use gpui::{
    AnyView, AppContext as _, Application, Bounds, WindowBounds, WindowDecorations, WindowOptions,
    px, size,
};

fn main() {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("[ade panic] {info}");
    }));

    let config = ade_core::AppConfig::load();
    let rt = ade_core::runtime::spawn(config);

    Application::new()
        .with_assets(gpui_component_assets::Assets)
        .run(move |cx| {
            gpui_component::init(cx);

            let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
            // Keep the WM/app-switcher title: transparent options blank it.
            let mut titlebar = gpui_component::TitleBar::title_bar_options();
            titlebar.title = Some("ADE — Agentic IDE".into());
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
                app_id: Some("ade".to_string()),
                window_min_size: Some(size(px(640.), px(480.))),
                ..Default::default()
            };

            cx.open_window(options, |window, cx| {
                let content = cx.new(|cx| app::AdeApp::new(rt.clone(), window, cx));
                cx.new(|cx| gpui_component::Root::new(AnyView::from(content), window, cx))
            })
            .expect("open ADE window");

            cx.activate(true);
        });
}
