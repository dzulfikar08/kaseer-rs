//! Kaseer design tokens on the GPUI Kit theme, in Atur Terminal's design
//! language.
//!
//! This module is the only place where raw palette hex values live. Screens
//! read semantic roles from `cx.theme()`, never literals (Design Guides,
//! "Color and themes").
//!
//! Source: atur-terminal `crates/muxy-ui` (the bundled "Atur" scheme, plus
//! `theme/colors.rs`, `theme/metrics.rs` and `theme/elevation.rs`):
//! - Neutral monochrome. Canvas `#0c0c0c`, text `#ebebeb`. Every other
//!   neutral is the text colour at a fixed alpha over the canvas, pre-blended
//!   here so gpui-kit roles stay opaque: muted text 65%, dim text 40%, raised
//!   surface 8%, hover 6% on top of a surface, border 12%.
//! - The accent is light gray (`#e4e4e4`) with black on it. It marks state
//!   (the active chip, the selected rail item, focus, the primary action)
//!   and never decorates.
//! - States come from the scheme's ANSI slots: danger `#e07070` (1), ok
//!   `#6fbf8a` (2), warning `#d4b062` (3).
//! - Compact metrics: 12px body, 24px controls, 4/6/8/10px radii
//!   (`radius_sm`..`radius_xl`), hairline borders, no gradients.
//! - Inter for the UI (Atur draws the system UI face; a kiosk cannot rely on
//!   whatever the OS ships, so Inter is embedded) and IBM Plex Mono for key
//!   caps, metadata and the receipt. Money uses Inter with `tnum`.
//!
//! Category hues are the one place colour carries information on the
//! register: a product tile's left edge and its chip's dot share the
//! category's hue, so a cashier finds items by colour region. They are the
//! Atur scheme's own ANSI tones, so they sit on the canvas without fighting
//! the neutral chrome.

use gpui_kit::component::{Colorize as _, Theme, ThemeMode, ThemeTokens};
use gpui_kit::{App, Hsla, Pixels, px};

/// `#0c0c0c`: the Atur canvas.
const BG: &str = "#0c0c0c";
/// The rail and key bar: the canvas lifted by 3% so the chrome separates
/// from the work area without a visible tint.
const RAIL: &str = "#111111";
/// `surface` (text at 8% over the canvas): panels, cards, tiles.
const SURFACE: &str = "#1e1e1e";
/// `hover` (text at 6%) on top of a surface: hovered rows, nested blocks.
const SURFACE_2: &str = "#2a2a2a";
/// `border` (text at 12% over the canvas).
const BORDER: &str = "#272727";
/// A border that must read on a surface (inputs, outlined chips).
const BORDER_STRONG: &str = "#383838";
const TEXT: &str = "#ebebeb";
/// `fg_muted` (text at 65%).
const TEXT_MUTED: &str = "#9d9d9d";
/// `fg_dim` (text at 40%): captions only, never body copy.
const TEXT_DIM: &str = "#656565";
const ACCENT: &str = "#e4e4e4";
const ACCENT_HOVER: &str = "#f5f5f5";
/// `contrasting_foreground(accent)`: black on the light accent.
const ON_ACCENT: &str = "#000000";
const DANGER: &str = "#e07070";
const OK: &str = "#6fbf8a";
const WARNING: &str = "#d4b062";

/// Category hues, in category order (see the module note): ochre, red,
/// green, cyan, magenta, blue from the Atur scheme's ANSI palette.
const CATEGORY_HUES: [&str; 6] = [
    "#d4b062", "#e07070", "#6fbf8a", "#6fb8c4", "#b595c4", "#7d9fd0",
];

/// The app's embedded sans family. Matches the family name baked into the
/// TTFs under `assets/fonts`, which `install` registers with the text
/// system.
pub const FONT_SANS: &str = "Inter";

/// Money: the sans with `tnum` (see `tabular`), so amounts line up digit
/// for digit without reading as code.
pub const FONT_MONEY: &str = FONT_SANS;

/// Key caps, metadata and the receipt preview: things that are mono in the
/// physical world or in Atur's chrome (keyboard legends, thermal printer
/// output, counters).
pub const FONT_MONO: &str = "IBM Plex Mono";

/// Atur `Metrics`: the type scale and radii screens size themselves with.
pub mod metrics {
    use gpui_kit::{Pixels, px};

    pub const FONT_CAPTION: Pixels = px(10.);
    pub const FONT_FOOTNOTE: Pixels = px(11.);
    pub const FONT_BODY: Pixels = px(12.);
    pub const FONT_EMPHASIS: Pixels = px(13.);
    pub const FONT_HEADLINE: Pixels = px(14.);
    pub const FONT_TITLE: Pixels = px(16.);
    pub const FONT_DISPLAY: Pixels = px(20.);

    pub const RADIUS_SM: Pixels = px(4.);
    pub const RADIUS_MD: Pixels = px(6.);
    pub const RADIUS_LG: Pixels = px(8.);
    pub const RADIUS_XL: Pixels = px(10.);
}

/// The receipt: white paper, near-black ink. Functions because
/// `Hsla::parse_hex` is not `const`.
/// Touch sizes for the selling screens (register, payment, rail). A cashier
/// taps these with a finger on a 15" touch panel, so nothing tappable is
/// under 52px and selling text starts at 15px.
pub mod touch {
    use gpui_kit::{Pixels, px};

    /// Secondary taps: chips, cart actions, quick cash, rail items.
    pub const TAP: Pixels = px(56.);
    /// Cart quantity steppers, list rows.
    pub const TAP_SM: Pixels = px(52.);
    /// The one primary action on a screen (Pay, Complete sale).
    pub const TAP_XL: Pixels = px(84.);
    pub const RADIUS: Pixels = px(12.);
    pub const RADIUS_LG: Pixels = px(14.);

    pub const FONT_LABEL: Pixels = px(15.);
    pub const FONT_BODY: Pixels = px(16.);
    pub const FONT_ITEM: Pixels = px(17.);
    pub const FONT_TILE: Pixels = px(18.);
    pub const FONT_ACTION: Pixels = px(22.);
    pub const FONT_TOTAL: Pixels = px(38.);
    pub const FONT_DUE: Pixels = px(56.);

    /// Product tiles: at least this tall, and the grid adds columns only
    /// while each stays at least `TILE_MIN_W` wide.
    pub const TILE_H: Pixels = px(118.);
    pub const TILE_MIN_W: f32 = 168.;
    pub const RAIL_W: Pixels = px(88.);
    pub const CART_W: Pixels = px(440.);
}

pub fn receipt_paper() -> Hsla {
    token("#ffffff")
}

pub fn receipt_ink() -> Hsla {
    token("#111111")
}

/// Cards, panels, product tiles: Atur `radius_lg`. Buttons and inputs use
/// `theme.radius` (`radius_md`), modals `theme.radius_lg` (`radius_xl`).
pub const CARD_RADIUS: Pixels = metrics::RADIUS_LG;

/// Inter's default digits are proportional; money must always carry `tnum`
/// so amounts line up down a column.
pub fn tabular() -> gpui_kit::FontFeatures {
    gpui_kit::FontFeatures(std::sync::Arc::new(vec![("tnum".to_string(), 1)]))
}

fn token(hex: &str) -> Hsla {
    Hsla::parse_hex(hex).expect("valid Kaseer hex token")
}

/// The hover border on cards and the focus ring: the strong neutral, so
/// focus reads without the accent's full weight.
pub fn accent_dim() -> Hsla {
    token(TEXT_MUTED)
}

/// Atur `accent_soft` (accent at 10%): the fill behind active chips and the
/// F2 pay key.
pub fn accent_soft() -> Hsla {
    token(ACCENT).opacity(0.1)
}

/// A border that reads on a raised surface (outlined chips, key caps).
pub fn border_strong() -> Hsla {
    token(BORDER_STRONG)
}

/// Atur `fg_dim`: captions, counters and key legends.
pub fn text_dim() -> Hsla {
    token(TEXT_DIM)
}

/// A category's identity hue: its position in the catalog's category list,
/// or, for a name outside the list, a stable FNV-1a hash of the name. Both
/// paths are deterministic, so a tile never changes colour between renders.
pub fn category_hue(categories: &[String], name: &str) -> Hsla {
    let index = categories
        .iter()
        .position(|category| category == name)
        .unwrap_or_else(|| category_hash(name) as usize);
    token(CATEGORY_HUES[index % CATEGORY_HUES.len()])
}

fn category_hash(name: &str) -> u32 {
    name.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

/// The K-mark, served by the app's asset source at [`KMARK_PATH`]. Drawn in
/// the Atur accent: the chrome stays monochrome, so the mark does too.
pub const KMARK_PATH: &str = "brand/kmark.svg";
pub const KMARK_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="128" height="128"><path fill="#e4e4e4" d="M10 6h14v20.8L44.6 6H59L38.2 29.4 59.4 58H44.1L27.6 36.9 24 41.2V58H10Z"/></svg>"##;

/// Load the dark base theme, then repaint every role with the Atur palette.
/// Call once after `gpui_kit::init`, before opening windows.
pub fn install(cx: &mut App) {
    let fonts: Vec<std::borrow::Cow<'static, [u8]>> = vec![
        std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/Inter-Regular.ttf")),
        std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/Inter-Medium.ttf")),
        std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/Inter-SemiBold.ttf")),
        std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")),
        std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-SemiBold.ttf")),
    ];
    cx.text_system()
        .add_fonts(fonts)
        .expect("embedded fonts load");

    Theme::change(ThemeMode::Dark, None, cx);

    {
        let theme = Theme::global_mut(cx);

        theme.background = token(BG);
        theme.foreground = token(TEXT);
        theme.border = token(BORDER);
        theme.input = token(BORDER_STRONG);
        theme.ring = token(TEXT_MUTED);
        // Atur selects with the near-white selection background; at full
        // strength it would hide the text in an input, so it is softened.
        theme.selection = token(TEXT).opacity(0.22);
        theme.overlay = token("#000000").opacity(0.6);

        // Panels and cards read `theme.secondary` (Atur `surface`).
        theme.secondary = token(SURFACE);
        theme.secondary_hover = token(SURFACE_2);
        theme.secondary_active = token(SURFACE_2);
        theme.secondary_foreground = token(TEXT);
        // gpui-kit draws Secondary without a border, so its fill must stand
        // apart from the panel it sits on: one step above `surface`.
        theme.button_secondary = token(SURFACE_2);
        theme.button_secondary_foreground = token(TEXT);
        theme.button_secondary_hover = token("#333333");
        theme.button_secondary_active = token("#3a3a3a");
        theme.muted = token(SURFACE_2);
        theme.muted_foreground = token(TEXT_MUTED);
        theme.popover = token(SURFACE);
        theme.popover_foreground = token(TEXT);
        theme.list_hover = token(SURFACE_2);
        theme.list_active = token(ACCENT).opacity(0.1);
        theme.list_active_border = token(BORDER_STRONG);
        theme.table_hover = token(SURFACE_2);
        theme.table_active = token(ACCENT).opacity(0.1);
        theme.table_active_border = token(BORDER_STRONG);
        // Tables sit inside panels: rows take the panel's surface, the head
        // a step darker, striping off.
        theme.table = token(SURFACE);
        theme.table_even = token(SURFACE);
        theme.table_foot = token(SURFACE);
        theme.table_foot_foreground = token(TEXT_MUTED);
        theme.table_head = token(RAIL);
        theme.table_head_foreground = token(TEXT_MUTED);
        theme.table_row_border = token(BORDER);
        theme.tab_bar = token(RAIL);
        theme.tab_active = token(SURFACE);
        theme.tab_active_foreground = token(TEXT);
        theme.tab_foreground = token(TEXT_MUTED);
        theme.scrollbar = gpui_kit::transparent_black();
        theme.scrollbar_thumb = token(TEXT).opacity(0.18);
        theme.scrollbar_thumb_hover = token(TEXT).opacity(0.3);

        theme.primary = token(ACCENT);
        theme.primary_foreground = token(ON_ACCENT);
        theme.primary_hover = token(ACCENT_HOVER);
        theme.primary_active = token(TEXT_MUTED);
        // `apply_config` only falls back `button_primary` → `primary` while
        // loading a theme config; recoloring the role afterwards does not
        // revisit it, so the button roles are set explicitly.
        theme.button_primary = token(ACCENT);
        theme.button_primary_foreground = token(ON_ACCENT);
        theme.button_primary_hover = token(ACCENT_HOVER);
        theme.button_primary_active = token(TEXT_MUTED);
        theme.accent = token(ACCENT);
        theme.accent_foreground = token(ON_ACCENT);
        theme.link = token(TEXT);
        theme.link_hover = token(ACCENT_HOVER);
        theme.link_active = token(TEXT_MUTED);

        // The nav rail: muted items, the active one on a surface in full
        // text colour (Atur's sidebar selection).
        theme.sidebar = token(RAIL);
        theme.sidebar_foreground = token(TEXT_MUTED);
        theme.sidebar_border = token(BORDER);
        theme.sidebar_accent = token(SURFACE);
        theme.sidebar_accent_foreground = token(TEXT);
        theme.sidebar_primary = token(ACCENT);
        theme.sidebar_primary_foreground = token(ON_ACCENT);

        theme.title_bar = token(BG);
        theme.title_bar_border = token(BORDER);

        theme.success = token(OK);
        theme.success_foreground = token(ON_ACCENT);
        theme.success_hover = token(OK).lighten(0.08);
        theme.success_active = token(OK).darken(0.08);
        theme.danger = token(DANGER);
        theme.danger_foreground = token(ON_ACCENT);
        theme.danger_hover = token(DANGER).lighten(0.08);
        theme.danger_active = token(DANGER).darken(0.08);
        theme.warning = token(WARNING);
        theme.warning_foreground = token(ON_ACCENT);
        theme.warning_hover = token(WARNING).lighten(0.08);
        theme.warning_active = token(WARNING).darken(0.08);
        theme.info = token(TEXT_MUTED);
        theme.info_foreground = token(ON_ACCENT);

        theme.radius = metrics::RADIUS_MD;
        theme.radius_lg = metrics::RADIUS_XL;
        theme.shadow = true;

        theme.font_family = FONT_SANS.into();
        theme.font_size = px(13.);
        theme.mono_font_family = FONT_MONO.into();
        theme.mono_font_size = px(12.);

        // Buttons and other styled components read the stored token
        // projection, not the color roles, so re-project after recoloring.
        theme.tokens = ThemeTokens::from(&theme.colors);
    }

    Theme::sync_base(cx);
}

#[cfg(test)]
mod tests {
    use super::{CATEGORY_HUES, category_hash, category_hue, token};

    #[test]
    fn category_hue_follows_order_then_hash() {
        let categories: Vec<String> = ["Coffee", "Food"].map(String::from).to_vec();
        assert_eq!(category_hue(&categories, "Coffee"), token(CATEGORY_HUES[0]));
        assert_eq!(category_hue(&categories, "Food"), token(CATEGORY_HUES[1]));
        let unknown = category_hash("Bakery") as usize % CATEGORY_HUES.len();
        assert_eq!(category_hue(&categories, "Bakery"), token(CATEGORY_HUES[unknown]));
        assert_eq!(category_hue(&[], "Bakery"), category_hue(&[], "Bakery"));
    }
}
