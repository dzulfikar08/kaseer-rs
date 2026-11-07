//! App screens. Each screen composes themed components; the shell owns
//! screen navigation and the overlay layers.

pub mod customers;

pub mod inventory;
pub mod knowledge;
pub mod register;
pub mod reports;
pub mod settings;
pub mod shift;
pub mod training;

use gpui_kit::component::ActiveTheme as _;
use crate::theme::metrics;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, ClickEvent, ElementId, FontWeight, InteractiveElement as _, ParentElement as _,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

pub use customers::CustomersScreen;

pub use inventory::InventoryScreen;
pub use knowledge::KnowledgeScreen;
pub use register::RegisterScreen;
pub use reports::ReportsScreen;
pub use settings::SettingsScreen;
pub use shift::ShiftScreen;
pub use training::TrainingScreen;

/// Atur chip: a full pill. Inactive is a strong hairline outline with muted
/// text; active is the accent-soft fill with an accent outline and full
/// text, weight 500. `cx` is the screen's render context (derefs to `App`).
pub(crate) fn chip(
    cx: &App,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<gpui_kit::Div> {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    div()
        .id(id.into())
        .cursor_pointer()
        .flex_none()
        .h(px(24.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(999.))
        .border_1()
        .text_size(metrics::FONT_BODY)
        .map(|chip| {
            if active {
                chip.bg(crate::theme::accent_soft())
                    .border_color(theme.accent)
                    .text_color(theme.foreground)
                    .font_weight(FontWeight::MEDIUM)
            } else {
                chip.border_color(crate::theme::border_strong())
                    .text_color(muted)
                    .hover(move |style| style.border_color(muted))
            }
        })
        .on_click(on_click)
        .child(label.into())
}

/// One money figure in a ledger column: tabular digits in the sans,
/// never wrapping, so amounts line up digit for digit down a column.
pub(crate) fn money_text(value: impl Into<SharedString>) -> gpui_kit::Div {
    div()
        .font_family(crate::theme::FONT_MONEY)
        .font_features(crate::theme::tabular())
        .whitespace_nowrap()
        .child(value.into())
}

/// Atur panel: surface fill, hairline border, `radius_lg`, 12px padding,
/// laid out as a column. Screens add their own gap and children.
pub(crate) fn panel(cx: &App) -> gpui_kit::Div {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(12.))
        .rounded(crate::theme::CARD_RADIUS)
        .bg(theme.secondary)
        .border_1()
        .border_color(theme.border)
}

/// Atur section title: 11px, weight 500, muted, uppercase with a little
/// tracking. Never bold or large: the content is the hierarchy.
pub(crate) fn section_title(cx: &App, title: impl Into<SharedString>) -> gpui_kit::Div {
    let title: SharedString = title.into();
    div()
        .text_size(metrics::FONT_FOOTNOTE)
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(title.to_uppercase()))
}

/// A KPI tile: section-title caption over a 20px semibold tabular value.
pub(crate) fn stat_tile(
    cx: &App,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> gpui_kit::Div {
    panel(cx)
        .gap(px(4.))
        .child(section_title(cx, label))
        .child(
            money_text(value)
                .text_size(metrics::FONT_DISPLAY)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cx.theme().foreground),
        )
}

/// A key cap: mono legend in a strong hairline outline, `radius_sm`.
pub(crate) fn key_cap(label: impl Into<SharedString>) -> gpui_kit::Div {
    div()
        .flex_none()
        .px(px(4.))
        .rounded(metrics::RADIUS_SM)
        .border_1()
        .border_color(crate::theme::border_strong())
        .font_family(crate::theme::FONT_MONO)
        .text_size(metrics::FONT_CAPTION)
        .font_weight(FontWeight::MEDIUM)
        .child(label.into())
}
