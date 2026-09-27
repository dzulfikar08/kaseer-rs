//! Kaseer application shell: GPUI Kit bootstrap, theme install, and the
//! first window.

mod screens;
mod shell;
mod theme;

use gpui_kit::component::Root;
use gpui_kit::{
    AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, px, size,
};

fn main() {
    gpui_kit::application()
        // AllAssets: the full Lucide catalog. The default `Assets`
        // bundle serves only the 101 component icons, which silently
        // drops half the nav rail (store, timer, chart-line, package,
        // users, megaphone are outside the default list).
        .with_assets(gpui_kit::assets::AllAssets)
        .run(|cx| {
            // Component layers must exist before any themed view.
            gpui_kit::init(cx);
            theme::install(cx);

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(1440.), px(900.)),
                })),
                // The register cart panel (lines + totals + pay) needs
                // roughly 700px of column height; below that the flex
                // boxes collapse and hitboxes start overlapping. Hold a
                // floor instead of shipping a window that can be resized
                // into a broken register.
                window_min_size: Some(size(px(1200.), px(760.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Kaseer".into()),
                    ..Default::default()
                }),
                app_id: Some("kaseer".into()),
                ..Default::default()
            };

            cx.spawn(async move |cx| {
                cx.open_window(options, |window, cx| {
                    let view = cx.new(|cx| shell::AppShell::new(window, cx));
                    // First level on the window is Root: overlays, menus,
                    // notifications, focus traps.
                    cx.new(|cx| Root::new(view, window, cx))
                })
                .expect("failed to open the Kaseer window");
            })
            .detach();
        });
}
