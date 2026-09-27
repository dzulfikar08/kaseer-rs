//! Kaseer application shell: GPUI Kit bootstrap, theme install, and the
//! first window.

mod screens;
mod shell;
mod theme;

use gpui_kit::component::Root;
use gpui_kit::{
    AppContext as _, AssetSource, Bounds, Result, SharedString, TitlebarOptions, WindowBounds,
    WindowOptions, point, px, size,
};
use std::borrow::Cow;

/// Serves the embedded IBM Plex Sans TTFs next to gpui-kit's Lucide icon
/// catalog. `theme::install` registers the font bytes with the text
/// system; the asset source keeps the same bytes reachable by path so
/// tooling and future `@source` references resolve.
struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match path {
            "fonts/IBMPlexSans-Regular.ttf" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/fonts/IBMPlexSans-Regular.ttf"
            )))),
            "fonts/IBMPlexSans-SemiBold.ttf" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/fonts/IBMPlexSans-SemiBold.ttf"
            )))),
            _ => gpui_kit::assets::AllAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::AllAssets.list(path)?;
        for font in ["fonts/IBMPlexSans-Regular.ttf", "fonts/IBMPlexSans-SemiBold.ttf"] {
            if font.starts_with(path) {
                paths.push(font.into());
            }
        }
        Ok(paths)
    }
}

fn main() {
    gpui_kit::application()
        // AppAssets: the full Lucide catalog plus the embedded fonts. The
        // default `Assets` bundle serves only the 101 component icons,
        // which silently drops half the nav rail (store, timer, chart-line,
        // package, users, megaphone are outside the default list).
        .with_assets(AppAssets)
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
