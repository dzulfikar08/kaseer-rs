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

/// The web `.chip`: a full-pill toggle. Inactive is a hairline outline with
/// muted text; active is the amber fill with the dark-on-amber foreground,
/// semibold like the web's `.chip.active`. `cx` is the screen's render
/// context (derefs to `App`).
pub(crate) fn chip(
    cx: &App,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<gpui_kit::Div> {
    let theme = cx.theme();
    div()
        .id(id.into())
        .cursor_pointer()
        .flex_none()
        .h(px(28.))
        .px(px(13.))
        .flex()
        .items_center()
        .rounded(px(999.))
        .text_size(px(12.5))
        .bg(if active { theme.accent } else { gpui_kit::transparent_black() })
        .when(!active, |chip| {
            chip.border_1()
                .border_color(theme.border)
                .hover(|style| style.border_color(theme.muted_foreground))
        })
        .text_color(if active {
            theme.accent_foreground
        } else {
            theme.muted_foreground
        })
        .when(active, |chip| chip.font_weight(FontWeight::SEMIBOLD))
        .on_click(on_click)
        .child(label.into())
}
