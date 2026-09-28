//! Kaseer design tokens, ported from the kaseer-web SPA (`src/web/theme.css`)
//! onto the GPUI Kit theme. The web stylesheet is the approved pixel
//! reference; its `:root` values win over the older kaseer/DESIGN.md set.
//!
//! This module is the only place where raw palette hex values live. Screens
//! read semantic roles from `cx.theme()`, never literals (Design Guides,
//! "Color and themes").
//!
//! theme.css mapping (brand rollout 2026-09-28):
//! - midnight neutrals `#0a0a0f` `#14161f` `#1c1f2b`, border `#2a2f3c`
//! - text `#f9fafc`, muted `#98a2b3`
//! - single indigo accent `#6366f1` (dim `#4f46e5`), white foreground
//! - states: danger `#f87171`, ok emerald `#10b981`
//! - radius 9 buttons/inputs, 12 cards/panels, 14 modals (`radius_lg`)
//! - 'Inter' (embedded via `assets/fonts`), 14px base, tabular
//!   numerals on money

use gpui_kit::component::{Colorize as _, Theme, ThemeMode, ThemeTokens};
use gpui_kit::{App, Hsla, Pixels, px};

const BG: &str = "#0a0a0f";
const SURFACE: &str = "#14161f";
const SURFACE_2: &str = "#1c1f2b";
const BORDER: &str = "#2a2f3c";
const TEXT: &str = "#f9fafc";
const MUTED: &str = "#98a2b3";
const ACCENT: &str = "#6366f1";
const ACCENT_DIM: &str = "#4f46e5";
const DANGER: &str = "#f87171";
const OK: &str = "#10b981";
/// The web `.btn.primary`/`.chip.active` foreground: white keeps the
/// indigo readable.
const ON_ACCENT: &str = "#ffffff";

/// The app's embedded sans family. Matches the family name baked into the
/// TTFs under `assets/fonts`, which `install` registers with the text
/// system; a family set explicitly on the theme is used as-is.
pub const FONT_SANS: &str = "Inter";

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

/// Load the dark base theme, then repaint every surface role with the web
/// palette. Call once after `gpui_kit::init`, before opening windows.
pub fn install(cx: &mut App) {
    let fonts: Vec<std::borrow::Cow<'static, [u8]>> = vec![
        std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/Inter-Regular.ttf")),
        std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/Inter-SemiBold.ttf")),
    ];
    cx.text_system()
        .add_fonts(fonts)
        .expect("embedded Inter loads");

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
        theme.muted_foreground = token(MUTED);
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

        // The nav rail: web `.rail` paints the strip `--surface` with the
        // active item on `--surface-2`.
        theme.sidebar = token(SURFACE);
        theme.sidebar_foreground = token(MUTED);
        theme.sidebar_border = token(BORDER);
        theme.sidebar_accent = token(SURFACE_2);
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

        // The web ships 'Inter' with a system-ui fallback; the
        // embedded family covers it, so name it directly.
        theme.font_family = FONT_SANS.into();

        // Buttons and other styled components read the stored token
        // projection, not the color roles, so re-project after recoloring.
        theme.tokens = ThemeTokens::from(&theme.colors);
    }

    Theme::sync_base(cx);
}
