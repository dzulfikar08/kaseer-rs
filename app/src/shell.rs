//! The app shell: left navigation rail plus the retained active screen,
//! wrapping everything in the window's single overlay mount.
//!
//! State ownership follows the coding guide: the shell owns one entity per
//! screen, constructed lazily on first visit and retained for the window's
//! lifetime (a screen's ledger connection, inputs, and subscriptions must
//! survive navigation). Overlay mounting lives here, exactly once per
//! window — gpui-kit 0.6.6's `Root::render` does not mount the dialog,
//! sheet, and notification layers, and duplicate mounts would paint every
//! dialog twice.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::{ActiveTheme as _, Icon, Root, Sizable as _};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, px,
};

use crate::screens::{
    CustomersScreen, InventoryScreen, KnowledgeScreen, RegisterScreen, ReportsScreen,
    SettingsScreen, ShiftScreen, TrainingScreen,
};

/// One navigation entry: the shell's screen identity. Dashboard,
/// Campaigns, and Copilot moved to the web companion and are deleted
/// from this app; Training and Knowledge are the terminal's own
/// operation screens.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NavItem {
    Register,
    Shift,
    Inventory,
    Customers,
    Reports,
    Settings,
    Training,
    Knowledge,
}

/// Rail order after the web-companion split. Training and Knowledge are
/// the two slots that used to sit reserved at the bottom — now real
/// screens, so the rail renders every entry the same way.
const NAV_ITEMS: &[NavItem] = &[
    NavItem::Register,
    NavItem::Shift,
    NavItem::Inventory,
    NavItem::Customers,
    NavItem::Reports,
    NavItem::Settings,
    NavItem::Training,
    NavItem::Knowledge,
];

impl NavItem {
    fn label(self) -> &'static str {
        match self {
            NavItem::Register => "Register",
            NavItem::Shift => "Shift",
            NavItem::Inventory => "Inventory",
            NavItem::Customers => "Customers",
            NavItem::Reports => "Reports",
            NavItem::Settings => "Settings",
            NavItem::Training => "Training",
            NavItem::Knowledge => "Knowledge",
        }
    }

    fn icon(self) -> IconName {
        match self {
            NavItem::Register => IconName::Store,
            NavItem::Shift => IconName::Timer,
            NavItem::Inventory => IconName::Package,
            NavItem::Customers => IconName::Users,
            NavItem::Reports => IconName::ChartLine,
            NavItem::Settings => IconName::Settings,
            NavItem::Training => IconName::GraduationCap,
            NavItem::Knowledge => IconName::BookOpen,
        }
    }
}

/// One slot per screen; `None` until first visit.
struct ScreenSlots {
    register: Option<Entity<RegisterScreen>>,
    shift: Option<Entity<ShiftScreen>>,
    inventory: Option<Entity<InventoryScreen>>,
    customers: Option<Entity<CustomersScreen>>,
    reports: Option<Entity<ReportsScreen>>,
    settings: Option<Entity<SettingsScreen>>,
    training: Option<Entity<TrainingScreen>>,
    knowledge: Option<Entity<KnowledgeScreen>>,
}

impl ScreenSlots {
    fn empty() -> Self {
        Self {
            register: None,
            shift: None,
            inventory: None,
            customers: None,
            reports: None,
            settings: None,
            training: None,
            knowledge: None,
        }
    }
}

/// The window's content view: nav rail, active screen, overlay layers.
pub struct AppShell {
    active: NavItem,
    screens: ScreenSlots,
}

impl AppShell {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            active: NavItem::Register,
            screens: ScreenSlots::empty(),
        }
    }

    /// The retained entity for `item`, constructing it on first visit. All
    /// screens share the constructor shape `new(window, cx)`.
    fn screen(&mut self, item: NavItem, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match item {
            NavItem::Register => self
                .screens
                .register
                .get_or_insert_with(|| cx.new(|cx| RegisterScreen::new(window, cx)))
                .clone()
                .into_any_element(),
            NavItem::Shift => self
                .screens
                .shift
                .get_or_insert_with(|| cx.new(|cx| ShiftScreen::new(window, cx)))
                .clone()
                .into_any_element(),
            NavItem::Inventory => self
                .screens
                .inventory
                .get_or_insert_with(|| cx.new(|cx| InventoryScreen::new(window, cx)))
                .clone()
                .into_any_element(),
            NavItem::Customers => self
                .screens
                .customers
                .get_or_insert_with(|| cx.new(|cx| CustomersScreen::new(window, cx)))
                .clone()
                .into_any_element(),
            NavItem::Reports => self
                .screens
                .reports
                .get_or_insert_with(|| cx.new(|cx| ReportsScreen::new(window, cx)))
                .clone()
                .into_any_element(),
            NavItem::Settings => self
                .screens
                .settings
                .get_or_insert_with(|| cx.new(|cx| SettingsScreen::new(window, cx)))
                .clone()
                .into_any_element(),
            NavItem::Training => self
                .screens
                .training
                .get_or_insert_with(|| cx.new(|cx| TrainingScreen::new(window, cx)))
                .clone()
                .into_any_element(),
            NavItem::Knowledge => self
                .screens
                .knowledge
                .get_or_insert_with(|| cx.new(|cx| KnowledgeScreen::new(window, cx)))
                .clone()
                .into_any_element(),
        }
    }

    fn render_nav(&self, cx: &Context<Self>) -> impl IntoElement {
        let active = self.active;
        // The web `.rail`: a 76px strip, items 64px wide with the icon
        // stacked over a 10.5px label; the active item sits on surface-2 in
        // accent. No brand block — the web rail starts straight in.
        v_flex()
            .id("nav-rail")
            .w(px(76.))
            .h_full()
            .flex_shrink_0()
            .items_center()
            .pt(px(10.))
            .gap(px(2.))
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .children(NAV_ITEMS.iter().map(|item| {
                let item = *item;
                let selected = item == active;
                let (fg, bg) = if selected {
                    (
                        cx.theme().sidebar_accent_foreground,
                        cx.theme().sidebar_accent,
                    )
                } else {
                    (cx.theme().sidebar_foreground, gpui_kit::transparent_black())
                };
                v_flex()
                    .id(SharedString::from(format!("nav-{}", item.label().to_lowercase())))
                    .cursor_pointer()
                    .w(px(64.))
                    .pt(px(8.))
                    .pb(px(6.))
                    .items_center()
                    .gap(px(2.))
                    .rounded(px(10.))
                    .bg(bg)
                    .text_color(fg)
                    .hover(|style| {
                        if selected {
                            style
                        } else {
                            style.text_color(cx.theme().foreground)
                        }
                    })
                    .on_click(cx.listener(move |shell, _, window, cx| {
                        shell.navigate(item, window, cx);
                    }))
                    .child(
                        Icon::new(item.icon())
                            .with_size(px(17.))
                            .text_color(fg),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .child(item.label()),
                    )
            }))
    }

    /// Switch screens and wake the one becoming visible. Retained screens
    /// hold snapshots that hidden siblings invalidate; the register is the
    /// seam that matters (shift gate, stock, customer list).
    fn navigate(&mut self, item: NavItem, window: &mut Window, cx: &mut Context<Self>) {
        if self.active != item {
            self.active = item;
            if item == NavItem::Register
                && let Some(register) = self.screens.register.as_ref()
            {
                register.update(cx, |r, cx| r.activate(window, cx));
            }
            cx.notify();
        }
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let screen = self.screen(self.active, window, cx);
        h_flex()
            .id("app-shell")
            .size_full()
            .items_stretch()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_nav(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .size_full()
                    .overflow_hidden()
                    .child(screen),
            )
            // The window's single overlay mount: dialogs, sheets, and
            // notifications for every screen. Exactly one view per window
            // may call this (gpui-kit 0.6.6 mounts layers through the
            // content view; `Root::render` does not).
            .children(overlay_layers(window, cx))
    }
}

/// The dialog, sheet, and notification overlay layers, which the window's
/// content view must render itself: `Root::render` does not mount them
/// (gpui-kit 0.6.6). Exactly one view per window calls this; duplicate
/// calls would paint every dialog twice.
fn overlay_layers(window: &mut Window, cx: &mut App) -> Vec<AnyElement> {
    [
        Root::render_dialog_layer(window, cx).map(|layer| layer.into_any_element()),
        Root::render_sheet_layer(window, cx).map(|layer| layer.into_any_element()),
        Root::render_notification_layer(window, cx).map(|layer| layer.into_any_element()),
    ]
    .into_iter()
    .flatten()
    .collect()
}
