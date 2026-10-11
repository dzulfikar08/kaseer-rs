//! Kaseer application shell: GPUI Kit bootstrap, theme install, and the
//! first window.

mod kiosk;
mod menu_sync;
mod screens;
mod shell;
mod sync_client;
mod theme;

use gpui_kit::component::Root;
use gpui_kit::{
    AppContext as _, AssetSource, Bounds, KeyBinding, Result, SharedString, TitlebarOptions,
    WindowBounds,
    WindowOptions, point, px, size,
};
use std::borrow::Cow;

/// Serves the embedded Plus Jakarta Sans TTFs next to gpui-kit's Lucide icon
/// catalog. `theme::install` registers the font bytes with the text
/// system; the asset source keeps the same bytes reachable by path so
/// tooling and future `@source` references resolve.
struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match path {
            "fonts/PlusJakartaSans-Regular.ttf" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/fonts/PlusJakartaSans-Regular.ttf"
            )))),
            "fonts/PlusJakartaSans-SemiBold.ttf" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/fonts/PlusJakartaSans-SemiBold.ttf"
            )))),
            theme::KMARK_PATH => Ok(Some(Cow::Borrowed(theme::KMARK_SVG.as_bytes()))),
            _ => gpui_kit::assets::AllAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::AllAssets.list(path)?;
        for font in [
            "fonts/PlusJakartaSans-Regular.ttf",
            "fonts/PlusJakartaSans-SemiBold.ttf",
            theme::KMARK_PATH,
        ] {
            if font.starts_with(path) {
                paths.push(font.into());
            }
        }
        Ok(paths)
    }
}

/// Field kiosks must leave crash traces: every panic appends timestamp,
/// message, location, and a full backtrace to `<data dir>/crash.log`
/// (best-effort: a broken data dir must not turn a panic into a
/// double fault).
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let stamp = kaseer_core::chrono::Utc::now()
            .to_rfc3339_opts(kaseer_core::chrono::SecondsFormat::Millis, true);
        let location = info
            .location()
            .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let report = format!(
            "===== {stamp} panic at {location} =====\n{info}\n{}\n",
            std::backtrace::Backtrace::force_capture()
        );
        let log = std::path::Path::new(&kaseer_core::db::resolve_db_path())
            .parent()
            .map(|dir| dir.join("crash.log"));
        if let Some(log) = log {
            if let Some(dir) = log.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log)
            {
                use std::io::Write as _;
                let _ = file.write_all(report.as_bytes());
            }
        }
        eprintln!("{report}");
    }));
}

fn main() {
    install_panic_hook();
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

            // Global keyboard navigation (shell.rs holds the handlers).
            // Register keeps F1 and the pay F2; the rest hold the web
            // app's muscle memory where the screen survived the split.
            cx.bind_keys([
                KeyBinding::new("f1", crate::shell::GoRegister, None),
                KeyBinding::new("f2", crate::shell::PayCurrent, None),
                KeyBinding::new("f5", crate::shell::GoTraining, None),
                KeyBinding::new("f6", crate::shell::GoReports, None),
                KeyBinding::new("f7", crate::shell::GoShift, None),
                KeyBinding::new("f8", crate::shell::GoInventory, None),
                KeyBinding::new("f9", crate::shell::GoCustomers, None),
                KeyBinding::new("f10", crate::shell::GoKnowledge, None),
                KeyBinding::new("f11", crate::shell::GoSettings, None),
                KeyBinding::new("alt-]", crate::shell::NextScreen, None),
                KeyBinding::new("alt-[", crate::shell::PrevScreen, None),
            ]);

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
                    // Toast pump: a window-attached task drains the
                    // background loops' queued notifications outside
                    // any update cycle (see kiosk::install_toast_pump).
                    kiosk::install_toast_pump(window, cx);
                    let view = cx.new(|cx| shell::AppShell::new(window, cx));
                    // First level on the window is Root: overlays, menus,
                    // notifications, focus traps.
                    cx.new(|cx| Root::new(view, window, cx))
                })
                .expect("failed to open the Kaseer window");

                // Cloud sync: drain whatever the last session left queued
                // (respects the failure backoff window; see sync_client).
                cx.background_spawn(async move {
                    let _ = sync_client::drain_once(false);
                })
                .detach();

                // Menu pull: the cloud (enrolled) or the companion owns the
                // catalog: poll its revision now and on the source's tick
                // (see menu_sync).
                menu_sync::spawn_menu_poll(cx.background_executor().clone());

                // Kiosk presence + auto-update: heartbeat at boot and every
                // 5 minutes, update check at boot and every 6 hours. This
                // await is the process's lifetime (see kiosk).
                kiosk::kiosk_loop(cx).await;
            })
            .detach();
        });
}
