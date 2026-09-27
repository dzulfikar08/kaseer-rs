//! Kaseer design tokens, ported from kaseer/DESIGN.md onto the GPUI Kit theme.
//!
//! This module is the only place where raw palette hex values live. Screens
//! read semantic roles from `cx.theme()`, never literals (Design Guides,
//! "Color and themes").
//!
//! DESIGN.md mapping:
//! - warm charcoal neutrals `#101013` `#16161a` `#1b1b20` `#232329`
//! - text `#f2f0ec`, dim `#a8a49c` (faint `#7a766e` reserved for large text)
//! - single till-amber accent `#ffb454`, dark-on-amber foreground
//! - money states only: green `#5fbf77`, red `#e0654f`
//! - radius 10 cards/inputs, 14 modals (`radius_lg`)

use gpui_kit::component::{Colorize as _, Theme, ThemeMode, ThemeTokens};
use gpui_kit::{App, Hsla, px};

const CHARCOAL: &str = "#101013";
const CHARCOAL_1: &str = "#16161a";
const CHARCOAL_2: &str = "#1b1b20";
const CHARCOAL_3: &str = "#232329";
const TEXT: &str = "#f2f0ec";
const TEXT_DIM: &str = "#a8a49c";
const AMBER: &str = "#ffb454";
const MONEY_GREEN: &str = "#5fbf77";
const MONEY_RED: &str = "#e0654f";

fn token(hex: &str) -> Hsla {
    Hsla::parse_hex(hex).expect("valid Kaseer hex token")
}

/// Load the dark base theme, then repaint every surface role with Kaseer
/// tokens. Call once after `gpui_kit::init`, before opening windows.
pub fn install(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);

    {
        let theme = Theme::global_mut(cx);

        theme.background = token(CHARCOAL);
        theme.foreground = token(TEXT);
        theme.border = token(CHARCOAL_3);
        theme.input = token(CHARCOAL_3);
        theme.ring = token(AMBER);
        theme.selection = token(AMBER).opacity(0.25);

        theme.secondary = token(CHARCOAL_2);
        theme.secondary_hover = token(CHARCOAL_3);
        theme.secondary_active = token(CHARCOAL_3);
        theme.secondary_foreground = token(TEXT);
        theme.muted = token(CHARCOAL_2);
        theme.muted_foreground = token(TEXT_DIM);
        theme.popover = token(CHARCOAL_2);
        theme.popover_foreground = token(TEXT);

        theme.primary = token(AMBER);
        theme.primary_foreground = token(CHARCOAL);
        theme.primary_hover = token(AMBER).lighten(0.08);
        theme.primary_active = token(AMBER).darken(0.08);
        theme.accent = token(AMBER);
        theme.accent_foreground = token(CHARCOAL);
        theme.link = token(AMBER);
        theme.link_hover = token(AMBER).lighten(0.08);
        theme.link_active = token(AMBER).darken(0.08);

        theme.sidebar = token(CHARCOAL_1);
        theme.sidebar_foreground = token(TEXT);
        theme.sidebar_border = token(CHARCOAL_3);
        theme.sidebar_accent = token(CHARCOAL_3);
        theme.sidebar_accent_foreground = token(TEXT);
        theme.sidebar_primary = token(AMBER);
        theme.sidebar_primary_foreground = token(CHARCOAL);

        theme.title_bar = token(CHARCOAL);
        theme.title_bar_border = token(CHARCOAL_3);

        theme.success = token(MONEY_GREEN);
        theme.success_foreground = token(CHARCOAL);
        theme.success_hover = token(MONEY_GREEN).lighten(0.08);
        theme.success_active = token(MONEY_GREEN).darken(0.08);
        theme.danger = token(MONEY_RED);
        theme.danger_foreground = token(CHARCOAL);
        theme.danger_hover = token(MONEY_RED).lighten(0.08);
        theme.danger_active = token(MONEY_RED).darken(0.08);
        theme.warning = token(AMBER);
        theme.warning_foreground = token(CHARCOAL);
        theme.warning_hover = token(AMBER).lighten(0.08);
        theme.warning_active = token(AMBER).darken(0.08);

        // DESIGN.md radius: 10 for cards/inputs, 14 for modals. Chips and
        // numpad keys get their 8px treatment at the component layer.
        theme.radius = px(10.);
        theme.radius_lg = px(14.);

        // Buttons and other styled components read the stored token
        // projection, not the color roles, so re-project after recoloring.
        theme.tokens = ThemeTokens::from(&theme.colors);
    }

    Theme::sync_base(cx);
}
