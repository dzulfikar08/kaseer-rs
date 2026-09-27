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
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Root, Selectable as _, Sizable as _};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div, px,
};

use crate::screens::{
    CampaignsScreen, CopilotScreen, CustomersScreen, DashboardScreen, InventoryScreen,
    RegisterScreen, ReportsScreen, SettingsScreen, ShiftScreen,
};

/// One navigation entry: the shell's screen identity.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NavItem {
    Register,
    Shift,
    Dashboard,
    Reports,
    Inventory,
    Customers,
    Campaigns,
    Copilot,
    Settings,
}

/// Rail order mirrors kaseer-web's `App.tsx` NAV.
const NAV_ITEMS: &[NavItem] = &[
    NavItem::Register,
    NavItem::Shift,
    NavItem::Dashboard,
    NavItem::Reports,
    NavItem::Inventory,
    NavItem::Customers,
    NavItem::Campaigns,
    NavItem::Copilot,
    NavItem::Settings,
];

impl NavItem {
    fn label(self) -> &'static str {
        match self {
            NavItem::Register => "Register",
            NavItem::Shift => "Shift",
            NavItem::Dashboard => "Dashboard",
            NavItem::Reports => "Reports",
            NavItem::Inventory => "Inventory",
            NavItem::Customers => "Customers",
            NavItem::Campaigns => "Campaigns",
            NavItem::Copilot => "Copilot",
            NavItem::Settings => "Settings",
        }
    }

    fn icon(self) -> IconName {
        match self {
            NavItem::Register => IconName::Store,
            NavItem::Shift => IconName::Timer,
            NavItem::Dashboard => IconName::LayoutDashboard,
            NavItem::Reports => IconName::ChartLine,
            NavItem::Inventory => IconName::Package,
            NavItem::Customers => IconName::Users,
            NavItem::Campaigns => IconName::Megaphone,
            NavItem::Copilot => IconName::Bot,
            NavItem::Settings => IconName::Settings,
        }
    }
}

/// One slot per screen; `None` until first visit.
struct ScreenSlots {
    register: Option<Entity<RegisterScreen>>,
    shift: Option<Entity<ShiftScreen>>,
    dashboard: Option<Entity<DashboardScreen>>,
    reports: Option<Entity<ReportsScreen>>,
    inventory: Option<Entity<InventoryScreen>>,
    customers: Option<Entity<CustomersScreen>>,
    campaigns: Option<Entity<CampaignsScreen>>,
    copilot: Option<Entity<CopilotScreen>>,
    settings: Option<Entity<SettingsScreen>>,
}

impl ScreenSlots {
    fn empty() -> Self {
        Self {
            register: None,
            shift: None,
            dashboard: None,
            reports: None,
            inventory: None,
            customers: None,
            campaigns: None,
            copilot: None,
            settings: None,
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
            NavItem::Dashboard => self
                .screens
                .dashboard
                .get_or_insert_with(|| cx.new(|cx| DashboardScreen::new(window, cx)))
                .clone()
                .into_any_element(),
            NavItem::Reports => self
                .screens
                .reports
                .get_or_insert_with(|| cx.new(|cx| ReportsScreen::new(window, cx)))
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
            NavItem::Campaigns => self
                .screens
                .campaigns
                .get_or_insert_with(|| cx.new(|cx| CampaignsScreen::new(window, cx)))
                .clone()
                .into_any_element(),
            NavItem::Copilot => self
                .screens
                .copilot
                .get_or_insert_with(|| cx.new(|cx| CopilotScreen::new(window, cx)))
                .clone()
                .into_any_element(),
            NavItem::Settings => self
                .screens
                .settings
                .get_or_insert_with(|| cx.new(|cx| SettingsScreen::new(window, cx)))
                .clone()
                .into_any_element(),
        }
    }

    fn render_nav(&self, cx: &Context<Self>) -> impl IntoElement {
        let active = self.active;
        v_flex()
            .id("nav-rail")
            .w(px(168.))
            .h_full()
            .flex_shrink_0()
            .gap_1()
            .p_2()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                div()
                    .px_2()
                    .pb_2()
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().sidebar_primary)
                    .child("Kaseer"),
            )
            .children(NAV_ITEMS.iter().map(|item| {
                let item = *item;
                Button::new(SharedString::from(format!("nav-{}", item.label().to_lowercase())))
                    .label(item.label())
                    .icon(item.icon())
                    .ghost()
                    .small()
                    .selected(item == active)
                    .on_click(cx.listener(move |shell, _, window, cx| {
                        shell.navigate(item, window, cx);
                    }))
            }))
    }

    /// Switch screens and wake the one becoming visible. Retained screens
    /// hold snapshots that hidden siblings invalidate; the register is the
    /// seam that matters (shift gate, stock, customer list).
    fn navigate(&mut self, item: NavItem, window: &mut Window, cx: &mut Context<Self>) {
        if self.active != item {
            self.active = item;
            if item == NavItem::Register {
                if let Some(register) = self.screens.register.as_ref() {
                    register.update(cx, |r, cx| r.activate(window, cx));
                }
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
