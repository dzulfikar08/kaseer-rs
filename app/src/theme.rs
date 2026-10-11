//! Kaseer design tokens on the GPUI Kit theme.
//!
//! This module is the only place where raw palette hex values live. Screens
//! read semantic roles from `cx.theme()`, never literals (Design Guides,
//! "Color and themes").
//!
//! Canvas mapping (pencil-new.pen, 2026-09-29, the blue redesign board):
//! - Midnight canvas `#0A0A0F`, rail `#0F1526`, surfaces `#17233B`
//!   `#1F2C48`, hover `#26355A`
//! - borders translucent white `#ffffff14` / `#ffffff26`
//! - text `#F1F5F9`, dim `#94A3B8`, faint `#64748B`
//! - single Electric Blue accent `#3B82F6` (dim `#2563EB`: focus ring),
//!   white foreground on accent
//! - states: danger `#F87171`, ok emerald `#10B981`
//! - radius 9 buttons/inputs, 12 cards/panels, 14 modals (`radius_lg`)
//! - Plus Jakarta Sans (UI and money, `tnum`) + IBM Plex Mono (key caps,
//!   receipt only), embedded via `assets/fonts`, 14px base
//!
//! Revenue register (0.2.6):
//! - Category hues: six muted tones (ochre, clay, sage, teal, lavender,
//!   dusty rose) that read on navy without competing with the accent. They
//!   carry information, not decoration: a product tile's 4px left edge and
//!   the dot on its filter chip share the category's hue, so a cashier finds
//!   items by colour region. Assigned by category order; a category outside
//!   the list gets a stable hash of its name. Same values as kaseer-web.
//! - The deep K gradient (135deg #2563EB, #4F46E5, #7C3AED) is reserved
//!   for the register's total block, the one place the sale's money and the
//!   Pay action live. Nothing else in the app may use a gradient. The deep
//!   stops (not the lighter brand-board ones) keep white text at AA.
//! - The K-mark SVG lives here too, so brand hex never leaves this file.

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
/// Gray 900 from the brand board: ink on the white inverse Pay button.
const NAVY_INK: &str = "#0F172A";

/// Category hues, in category order (see the module note).
const CATEGORY_HUES: [&str; 6] = [
    "#D4A657", // ochre
    "#D98B6A", // clay
    "#8DB580", // sage
    "#5FB0BD", // teal
    "#A895D9", // lavender
    "#C98AA8", // dusty rose
];

/// Deep K gradient stops for the total block (135deg).
const TOTAL_GRADIENT: [&str; 3] = ["#2563EB", "#4F46E5", "#7C3AED"];
/// The web `.btn.primary`/`.chip.active` foreground: white keeps the
/// Electric Blue readable.
const ON_ACCENT: &str = "#ffffff";

/// The app's embedded sans family. Matches the family name baked into the
/// TTFs under `assets/fonts`, which `install` registers with the text
/// system; a family set explicitly on the theme is used as-is.
/// Plus Jakarta Sans (Tokotype, OFL): drawn in Jakarta for the city's
/// identity, which suits a register built for Indonesian shops; its open
/// counters read well at tile size.
pub const FONT_SANS: &str = "Plus Jakarta Sans";

/// Money: the sans with `tnum` (see `tabular`), so amounts line up digit
/// for digit without reading as code.
pub const FONT_MONEY: &str = FONT_SANS;

/// Key caps and the receipt preview only: things that are mono in the
/// physical world (keyboard legends, thermal printer output).
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

/// Web money cells set `font-variant-numeric: tabular-nums`; Plus Jakarta
/// Sans ships `tnum` (its default digits are proportional), so money must
/// always carry this.
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

/// The total block's two gradient layers. GPUI gradients take two stops,
/// so the three-stop K gradient is a base layer (blue to indigo, holding
/// indigo past the midpoint) under a layer that fades violet in from the
/// midpoint. Painted as two stacked fills, the result matches the CSS
/// `linear-gradient(135deg, #2563EB, #4F46E5, #7C3AED)`.
pub fn total_gradient() -> [gpui_kit::Background; 2] {
    let [blue, indigo, violet] = TOTAL_GRADIENT.map(token);
    [
        gpui_kit::linear_gradient(
            135.,
            gpui_kit::linear_color_stop(blue, 0.),
            gpui_kit::linear_color_stop(indigo, 0.5),
        ),
        gpui_kit::linear_gradient(
            135.,
            gpui_kit::linear_color_stop(violet.opacity(0.), 0.5),
            gpui_kit::linear_color_stop(violet, 1.),
        ),
    ]
}

/// The card border (`#ffffff14`) pre-blended over the tile surface. Product
/// tiles paint their category hue under the border, where a translucent
/// border would turn the hue's colour; this opaque twin keeps the outline
/// identical to every other card.
pub fn tile_border() -> Hsla {
    token("#29344A")
}

/// Ink on the white inverse button inside the total block.
pub fn navy_ink() -> Hsla {
    token(NAVY_INK)
}

/// White on the gradient: every stop passes AA for normal text (5.2:1 at
/// the lightest stop).
pub fn on_gradient() -> Hsla {
    token(ON_ACCENT)
}

/// The K-mark (kaseer-site `public/kmark.svg`), served by the app's asset
/// source at [`KMARK_PATH`]. Rendered as an image so the gradient survives
/// (an `svg()` element would tint it to one colour). 128px intrinsic size
/// so the rasterized mark stays crisp at the rail's 30px.
pub const KMARK_PATH: &str = "brand/kmark.svg";
pub const KMARK_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="128" height="128"><defs><linearGradient id="k" x1="8" y1="58" x2="56" y2="6" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#8B5CF6"/><stop offset=".52" stop-color="#6366F1"/><stop offset="1" stop-color="#3B82F6"/></linearGradient></defs><path fill="url(#k)" d="M10 6h14v20.8L44.6 6H59L38.2 29.4 59.4 58H44.1L27.6 36.9 24 41.2V58H10Z"/></svg>"##;

/// Load the dark base theme, then repaint every surface role with the web
/// palette. Call once after `gpui_kit::init`, before opening windows.
pub fn install(cx: &mut App) {
    let fonts: Vec<std::borrow::Cow<'static, [u8]>> = vec![
        std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/PlusJakartaSans-Regular.ttf")),
        std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/PlusJakartaSans-Medium.ttf")),
        std::borrow::Cow::Borrowed(include_bytes!("../assets/fonts/PlusJakartaSans-SemiBold.ttf")),
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
        // from this role below: leave it unset and the default dark
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

        // Plus Jakarta Sans for the app (money via `tnum`); Plex Mono for
        // key caps and the receipt. Both families are embedded above.
        theme.font_family = FONT_SANS.into();
        theme.mono_font_family = FONT_MONO.into();

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
