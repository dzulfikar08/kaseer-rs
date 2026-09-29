//! Kaseer design tokens on the GPUI Kit theme.
//!
//! This module is the only place where raw palette hex values live. Screens
//! read semantic roles from `cx.theme()`, never literals (Design Guides,
//! "Color and themes").
//!
//! Canvas mapping (pencil-new.pen, 2026-09-29 — the blue redesign board):
//! - Midnight canvas `#0A0A0F`, rail `#0F1526`, surfaces `#17233B`
//!   `#1F2C48`, hover `#26355A`
//! - borders translucent white `#ffffff14` / `#ffffff26`
//! - text `#F1F5F9`, dim `#94A3B8`, faint `#64748B`
//! - single Electric Blue accent `#3B82F6` (dim `#2563EB` — focus ring),
//!   white foreground on accent
//! - states: danger `#F87171`, ok emerald `#10B981`
//! - radius 9 buttons/inputs, 12 cards/panels, 14 modals (`radius_lg`)
//! - IBM Plex Sans + IBM Plex Mono (embedded via `assets/fonts`), 14px
//!   base, money in Plex Mono

use gpui_kit::component::{Colorize as _, Theme, ThemeMode, ThemeTokens};
use gpui_kit::{App, Hsla, Pixels, px};

const BG: &str = "#0A0A0F";
const RAIL: &str = "#0F1526";
const SURFACE: &str = "#17233B";
const SURFACE_2: &str = "#1F2C48";
const HOVER: &str = "#26355A";
const BORDER: &str = "#ffffff14";
const BORDER_STRONG: &str = "#ffffff26";
const TEXT: &str = "#F1F5F9";
const TEXT_DIM: &str = "#94A3B8";
const TEXT_FAINT: &str = "#64748B";
const ACCENT: &str = "#3B82F6";
const ACCENT_DIM: &str = "#2563EB";
const DANGER: &str = "#F87171";
const OK: &str = "#10B981";
/// The web `.btn.primary`/`.chip.active` foreground: white keeps the
/// Electric Blue readable.
const ON_ACCENT: &str = "#ffffff";

/// The app's embedded sans family. Matches the family name baked into the
/// TTFs under `assets/fonts`, which `install` registers with the text
/// system; a family set explicitly on the theme is used as-is.
pub const FONT_SANS: &str = "IBM Plex Sans";

/// Money, key caps, and tabular readouts — the canvas `font-mono`.
pub const FONT_MONO: &str = "IBM Plex Mono";

/// The web `.receipt`: white paper, near-black ink. Functions because
/// `Hsla::parse_hex` is not `const`.
pub fn receipt_paper() -> Hsla {
    token("#ffffff")
}

pub fn receipt_ink() -> Hsla {
    token("#111111")
}

/// Cards, panels, product tiles: the web `.panel`/`.catcard` 12px radius.
/// Buttons and inputs stay on `theme.radius` (9px) because gpui-kit reads
/// the theme radius for both, and the two genuinely differ on the web.
pub const CARD_RADIUS: Pixels = px(12.);

/// Web money cells set `font-variant-numeric: tabular-nums`; the embedded
/// Inter honors `tnum`, so digits line up without falling back to
/// a monospace face.
pub fn tabular() -> gpui_kit::FontFeatures {
    gpui_kit::FontFeatures(std::sync::Arc::new(vec![("tnum".to_string(), 1)]))
}

fn token(hex: &str) -> Hsla {
    Hsla::parse_hex(hex).expect("valid Kaseer hex token")
}

/// `--accent-dim`: the hover border on product cards and the focus ring.
/// Screens read it here because no gpui-kit color role carries it.
pub fn accent_dim() -> Hsla {
    token(ACCENT_DIM)
}

/// `--accent-soft`: the tinted fill behind active chips and the F2 pay key.
pub fn accent_soft() -> Hsla {
    token("#3B82F626")
}

/// Load the dark base theme, then repaint every surface role with the web
/// palette. Call once after `gpui_kit::init`, before opening windows.
pub fn install(cx: &mut App) {
    let fonts: Vec<std::borrow::Cow<'static, [u8]>> = vec![
        std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf")),
        std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf")),
        std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf")),
        std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")),
        std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-SemiBold.ttf")),
    ];
    cx.text_system()
        .add_fonts(fonts)
        .expect("embedded IBM Plex loads");

    Theme::change(ThemeMode::Dark, None, cx);

    {
        let theme = Theme::global_mut(cx);

        theme.background = token(BG);
        theme.foreground = token(TEXT);
        theme.border = token(BORDER);
        theme.input = token(BORDER);
        theme.ring = token(ACCENT_DIM);
        theme.selection = token(ACCENT).opacity(0.25);
        theme.overlay = token("#05070a").opacity(0.62);

        // Panels and cards: the web `.panel`/`.catcard` surface. Screens
        // read `theme.secondary` for these, so it carries `--surface`;
        // the Secondary *button* fill comes from the independent
        // `button_secondary` tokens below (web `.btn` paints surface-2).
        theme.secondary = token(SURFACE);
        theme.secondary_hover = token(SURFACE);
        theme.secondary_active = token(SURFACE_2);
        theme.secondary_foreground = token(TEXT);
        theme.button_secondary = token(SURFACE_2);
        theme.button_secondary_foreground = token(TEXT);
        theme.button_secondary_hover = token(SURFACE_2);
        theme.button_secondary_active = token(SURFACE_2);
        theme.muted = token(SURFACE_2);
        theme.muted_foreground = token(TEXT_DIM);
        theme.popover = token(SURFACE);
        theme.popover_foreground = token(TEXT);

        theme.primary = token(ACCENT);
        theme.primary_foreground = token(ON_ACCENT);
        theme.primary_hover = token(ACCENT_DIM);
        theme.primary_active = token(ACCENT_DIM);
        // `apply_config` only falls back `button_primary` → `primary` while
        // loading a theme config; recoloring the role afterwards does not
        // revisit it. Button reads `tokens.button_primary`, which is copied
        // from this role below — leave it unset and the default dark
        // palette's near-white survives into the Pay button.
        theme.button_primary = token(ACCENT);
        theme.button_primary_foreground = token(ON_ACCENT);
        theme.button_primary_hover = token(ACCENT_DIM);
        theme.button_primary_active = token(ACCENT_DIM);
        theme.accent = token(ACCENT);
        theme.accent_foreground = token(ON_ACCENT);
        theme.link = token(ACCENT);
        theme.link_hover = token(ACCENT_DIM);
        theme.link_active = token(ACCENT_DIM);

        // The nav rail: web `.rail` paints the strip `--rail` with the
        // active item on the hover surface in accent.
        theme.sidebar = token(RAIL);
        theme.sidebar_foreground = token(TEXT_DIM);
        theme.sidebar_border = token(BORDER);
        theme.sidebar_accent = token(HOVER);
        theme.sidebar_accent_foreground = token(ACCENT);
        theme.sidebar_primary = token(ACCENT);
        theme.sidebar_primary_foreground = token(ON_ACCENT);

        theme.title_bar = token(BG);
        theme.title_bar_border = token(BORDER);

        theme.success = token(OK);
        theme.success_foreground = token(BG);
        theme.success_hover = token(OK).lighten(0.08);
        theme.success_active = token(OK).darken(0.08);
        theme.danger = token(DANGER);
        theme.danger_foreground = token(BG);
        theme.danger_hover = token(DANGER).lighten(0.08);
        theme.danger_active = token(DANGER).darken(0.08);
        theme.warning = token(ACCENT);
        theme.warning_foreground = token(ON_ACCENT);
        theme.warning_hover = token(ACCENT).lighten(0.08);
        theme.warning_active = token(ACCENT).darken(0.08);

        // theme.css radius: 9px buttons/inputs, 14px modals. Cards and
        // panels use CARD_RADIUS (12px) at the component layer, chips are
        // full pills.
        theme.radius = px(9.);
        theme.radius_lg = px(14.);

        // The canvas sets IBM Plex Sans as the app family and Plex Mono
        // for money/readouts; both families are embedded above.
        theme.font_family = FONT_SANS.into();
        theme.mono_font_family = FONT_MONO.into();

        // Buttons and other styled components read the stored token
        // projection, not the color roles, so re-project after recoloring.
        theme.tokens = ThemeTokens::from(&theme.colors);
    }

    Theme::sync_base(cx);
}
