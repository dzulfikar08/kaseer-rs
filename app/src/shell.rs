//! The app shell: left navigation rail plus the retained active screen,
//! wrapping everything in the window's single overlay mount.
//!
//! State ownership follows the coding guide: the shell owns one entity per
//! screen, constructed lazily on first visit and retained for the window's
//! lifetime (a screen's ledger connection, inputs, and subscriptions must
//! survive navigation). Dialogs, sheets and notifications are mounted by
//! `Root` itself (gpui-kit 0.9 root plugins), not by the shell.

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _};
use crate::theme::{FONT_MONO, metrics};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    actions, AnyElement, AppContext as _, Context, Entity, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, img, px,
};

use crate::screens::register::RegisterEvent;
use crate::screens::{
    CustomersScreen, InventoryScreen, KnowledgeScreen, RegisterScreen, ReportsScreen,
    SettingsScreen, ShiftScreen, TrainingScreen,
};

actions!(
    kaseer,
    [
        GoRegister,
        GoShift,
        GoInventory,
        GoCustomers,
        GoReports,
        GoSettings,
        GoTraining,
        GoKnowledge,
        NextScreen,
        PrevScreen,
        PayCurrent,
    ]
);

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
/// the two slots that used to sit reserved at the bottom: now real
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

    /// Global function-key shortcut. The register keeps the terminal's
    /// F1; the rest hold the web app's muscle memory where the screen
    /// survived the web-companion split.
    fn fkey(self) -> &'static str {
        match self {
            NavItem::Register => "F1",
            NavItem::Training => "F5",
            NavItem::Reports => "F6",
            NavItem::Shift => "F7",
            NavItem::Inventory => "F8",
            NavItem::Customers => "F9",
            NavItem::Knowledge => "F10",
            NavItem::Settings => "F11",
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
    /// Keyboard home: focused once at startup so the shell's action
    /// handlers receive F-key dispatch from anywhere (text inputs bubble
    /// unhandled keys up the focus path to here).
    focus: FocusHandle,
    /// Re-homes focus when the focused element leaves the tree (a screen
    /// swapping its body, e.g. Training's start card becoming the lesson
    /// view). Without it the orphaned focus swallows every F-key.
    _focus_lost: Subscription,
    /// The register's requests (its "Open shift" control), subscribed once
    /// when the register entity is first built.
    _register_events: Option<Subscription>,
}

impl AppShell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let _focus_lost = cx.on_focus_lost(window, |shell, window, cx| {
            shell.focus.focus(window, cx);
        });
        Self {
            active: NavItem::Register,
            screens: ScreenSlots::empty(),
            focus,
            _focus_lost,
            _register_events: None,
        }
    }

    /// The retained entity for `item`, constructing it on first visit. All
    /// screens share the constructor shape `new(window, cx)`.
    fn screen(&mut self, item: NavItem, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match item {
            NavItem::Register => {
                if self.screens.register.is_none() {
                    let register = cx.new(|cx| RegisterScreen::new(window, cx));
                    self._register_events = Some(cx.subscribe_in(
                        &register,
                        window,
                        |shell, _, event, window, cx| match event {
                            RegisterEvent::OpenShift => shell.navigate(NavItem::Shift, window, cx),
                        },
                    ));
                    self.screens.register = Some(register);
                }
                self.screens
                    .register
                    .clone()
                    .expect("register built above")
                    .into_any_element()
            }
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
        // Atur's sidebar: a narrow rail-coloured strip, muted icon + label
        // items, the selected one on a surface in full text colour. The
        // function keys live in the key bar, so the rail stays quiet. The
        // flat K-mark heads it.
        v_flex()
            .id("nav-rail")
            .w(px(68.))
            .h_full()
            .flex_shrink_0()
            .items_center()
            .pt(px(10.))
            .gap(px(2.))
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                img(crate::theme::KMARK_PATH)
                    .size(px(20.))
                    .flex_none()
                    .mt(px(2.))
                    .mb(px(14.)),
            )
            .children(NAV_ITEMS.iter().map(|item| {
                let item = *item;
                let selected = item == active;
                let theme = cx.theme();
                let (fg, bg) = if selected {
                    (theme.sidebar_accent_foreground, theme.sidebar_accent)
                } else {
                    (theme.sidebar_foreground, gpui_kit::transparent_black())
                };
                let (hover_fg, hover_bg) = (theme.foreground, theme.secondary);
                v_flex()
                    .id(SharedString::from(format!(
                        "nav-{}",
                        item.label().to_lowercase()
                    )))
                    .cursor_pointer()
                    .w(px(60.))
                    .py(px(7.))
                    .items_center()
                    .gap(px(4.))
                    .rounded(metrics::RADIUS_MD)
                    .bg(bg)
                    .when(selected, |item| item.border_1().border_color(theme.border))
                    .text_color(fg)
                    .hover(move |style| {
                        if selected {
                            style
                        } else {
                            style.text_color(hover_fg).bg(hover_bg)
                        }
                    })
                    .on_click(cx.listener(move |shell, _, window, cx| {
                        shell.navigate(item, window, cx);
                    }))
                    .child(Icon::new(item.icon()).with_size(px(16.)).text_color(fg))
                    .child(
                        div()
                            .text_size(metrics::FONT_CAPTION)
                            .whitespace_nowrap()
                            .when(selected, |label| label.font_weight(FontWeight::MEDIUM))
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
            // The outgoing screen's focused input leaves the tree with it,
            // and an orphaned focus swallows every later F-key. Park focus
            // on the shell so the global keymap keeps working; the register
            // takes it back for its scan box in `activate`.
            self.focus.focus(window, cx);
            if item == NavItem::Register
                && let Some(register) = self.screens.register.as_ref()
            {
                register.update(cx, |r, cx| r.activate(window, cx));
            }
            cx.notify();
        }
    }

    /// The web `.keybar` renderer lives here with the other shell helpers.
    /// (moved: trait impls cannot carry extra methods)
    /// Alt+] / Alt+[ walk the rail in order, wrapping at the ends.
    fn cycle(&mut self, dir: isize, window: &mut Window, cx: &mut Context<Self>) {
        let count = NAV_ITEMS.len() as isize;
        let current = NAV_ITEMS
            .iter()
            .position(|item| *item == self.active)
            .unwrap_or(0) as isize;
        let next = NAV_ITEMS[((current + dir).rem_euclid(count)) as usize];
        self.navigate(next, window, cx);
    }

    /// Atur's status bar: a 28px rail-coloured strip of mono key caps and
    /// labels. Every binding is clickable and mirrors the global keymap, so
    /// the keyboard map is discoverable without docs.
    fn render_key_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active;
        let theme = cx.theme().clone();
        let hover_bg = theme.secondary;
        let caps = NAV_ITEMS.iter().map(|item| {
            let item = *item;
            let selected = item == active;
            h_flex()
                .id(SharedString::from(format!("key-{}", item.label().to_lowercase())))
                .cursor_pointer()
                .h(px(20.))
                .gap(px(6.))
                .items_center()
                .px(px(6.))
                .rounded(metrics::RADIUS_SM)
                .when(selected, |cap| cap.bg(theme.secondary))
                .hover(move |style| style.bg(hover_bg))
                .on_click(cx.listener(move |shell, _, window, cx| {
                    shell.navigate(item, window, cx);
                }))
                .child(
                    crate::screens::key_cap(item.fkey())
                        .text_color(if selected { theme.foreground } else { theme.muted_foreground }),
                )
                .child(
                    div()
                        .text_size(metrics::FONT_FOOTNOTE)
                        .text_color(if selected { theme.foreground } else { theme.muted_foreground })
                        .child(item.label()),
                )
        });
        let (accent, accent_fg, accent_hover) =
            (theme.primary, theme.primary_foreground, theme.primary_hover);
        h_flex()
            .id("key-bar")
            .flex_shrink_0()
            .h(px(28.))
            .items_center()
            .gap(px(2.))
            .px(px(6.))
            .bg(theme.sidebar)
            .border_t_1()
            .border_color(theme.sidebar_border)
            .children(caps)
            .child(
                div()
                    .ml_auto()
                    .mr(px(10.))
                    .font_family(FONT_MONO)
                    .text_size(metrics::FONT_CAPTION)
                    .text_color(crate::theme::text_dim())
                    .child("Alt+] / Alt+[ cycle"),
            )
            .child(
                h_flex()
                    .id("key-pay")
                    .cursor_pointer()
                    .h(px(20.))
                    .gap(px(6.))
                    .items_center()
                    .px(px(8.))
                    .rounded(metrics::RADIUS_SM)
                    .bg(accent)
                    .text_color(accent_fg)
                    .hover(move |style| style.bg(accent_hover))
                    .on_click(cx.listener(|shell, _, window, cx| {
                        if shell.active == NavItem::Register
                            && let Some(register) = shell.screens.register.as_ref()
                        {
                            register.update(cx, |r, cx| r.open_pay(window, cx));
                        }
                    }))
                    .child(
                        crate::screens::key_cap("F2").border_color(accent_fg.opacity(0.25)),
                    )
                    .child(
                        div()
                            .text_size(metrics::FONT_FOOTNOTE)
                            .font_weight(FontWeight::MEDIUM)
                            .child("Pay"),
                    ),
            )
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let screen = self.screen(self.active, window, cx);
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            // Global keyboard navigation. Text inputs only see keys they
            // handle; everything else bubbles up the focus path to here.
            .on_action(cx.listener(|shell, _: &GoRegister, window, cx| shell.navigate(NavItem::Register, window, cx)))
            .on_action(cx.listener(|shell, _: &GoShift, window, cx| shell.navigate(NavItem::Shift, window, cx)))
            .on_action(cx.listener(|shell, _: &GoInventory, window, cx| shell.navigate(NavItem::Inventory, window, cx)))
            .on_action(cx.listener(|shell, _: &GoCustomers, window, cx| shell.navigate(NavItem::Customers, window, cx)))
            .on_action(cx.listener(|shell, _: &GoReports, window, cx| shell.navigate(NavItem::Reports, window, cx)))
            .on_action(cx.listener(|shell, _: &GoSettings, window, cx| shell.navigate(NavItem::Settings, window, cx)))
            .on_action(cx.listener(|shell, _: &GoTraining, window, cx| shell.navigate(NavItem::Training, window, cx)))
            .on_action(cx.listener(|shell, _: &GoKnowledge, window, cx| shell.navigate(NavItem::Knowledge, window, cx)))
            .on_action(cx.listener(|shell, _: &NextScreen, window, cx| shell.cycle(1, window, cx)))
            .on_action(cx.listener(|shell, _: &PrevScreen, window, cx| shell.cycle(-1, window, cx)))
            .on_action(cx.listener(|shell, _: &PayCurrent, window, cx| {
                if shell.active == NavItem::Register
                    && let Some(register) = shell.screens.register.as_ref()
                {
                    register.update(cx, |r, cx| r.open_pay(window, cx));
                }
            }))
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(self.render_nav(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .size_full()
                            .overflow_hidden()
                            .child(screen),
                    ),
            )
            .child(self.render_key_bar(cx))
    }

}
