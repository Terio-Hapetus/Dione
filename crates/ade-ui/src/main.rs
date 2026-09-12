mod app;
mod views;
mod vm_thread;

use gpui::{
    AnyView, AppContext as _, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
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
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("ADE — Agentic IDE".into()),
                    ..Default::default()
                }),
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
