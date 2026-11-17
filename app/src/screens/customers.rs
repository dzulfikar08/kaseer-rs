//! Customers screen: search, sell-to, points adjust, and the add-customer
//! dialog, ported 1:1 from the web `customers.tsx` and sized for a touch
//! screen: a 60px search bar, 72px rows, 52px − / + points steppers and a
//! big Sell to button.

use std::sync::Mutex;

use gpui_kit::assets::IconName;
use gpui_kit::base::{StyledExt as _, h_flex, v_flex};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    button::Button,
    button::ButtonVariants as _,
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::Input,
    input::InputEvent,
    input::InputState,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, Global, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px,
};
use kaseer_core::db::{open_repo, resolve_db_path};
use kaseer_core::reports::ms_to_iso;
use kaseer_core::repos::{Customer, Repo};

use crate::theme::touch;

/// App-wide pointer to the customer the register will sell to.
///
/// Written by this screen's "Sell to" action; read by the Register screen
/// via [`SelectedCustomer::get`]. A GPUI `Global` (coding guides, "State
/// ownership": shared application state may live as a global) because the
/// selection must survive screen navigation while the screens themselves
/// come and go.
#[derive(Debug, Default)]
pub struct SelectedCustomer(Option<Customer>);

impl SelectedCustomer {
    /// The customer currently selected for the register, if any.
    pub fn get(cx: &App) -> Option<Customer> {
        cx.try_global::<Self>().and_then(|s| s.0.clone())
    }

    /// Replace the handoff; `None` marks it consumed. Register clears it
    /// once the pick has been attached to the cart.
    pub fn set(cx: &mut App, customer: Option<Customer>) {
        cx.set_global(Self(customer));
    }
}

impl Global for SelectedCustomer {}

/// Points moved by one ± tap, matching the web screen.
const POINTS_STEP: i64 = 10;

/// The customers panel: search, list, add-customer dialog.
pub struct CustomersScreen {
    /// `None` when the database could not be opened; the panel then shows
    /// the reason and every action reports the failure.
    repo: Option<Mutex<Repo>>,
    customers: Vec<Customer>,
    search: Entity<InputState>,
    filter: String,
    name_input: Entity<InputState>,
    phone_input: Entity<InputState>,
    load_error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl CustomersScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (repo, load_error) = match open_repo(&resolve_db_path()) {
            Ok(repo) => (Some(Mutex::new(repo)), None),
            Err(e) => (None, Some(e.to_string())),
        };

        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search name or phone"));
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Customer name"));
        let phone_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("WhatsApp number, e.g. 0812…"));

        let mut this = Self {
            repo,
            customers: Vec::new(),
            search,
            filter: String::new(),
            name_input,
            phone_input,
            load_error,
            _subscriptions: Vec::new(),
        };

        this._subscriptions.push(cx.subscribe_in(
            &this.search.clone(),
            window,
            |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    // The owner keeps the live filter; render only derives.
                    this.filter = this.search.read(cx).value().trim().to_lowercase();
                    cx.notify();
                }
            },
        ));
        // Enter in the add dialog arrives as the dialog's confirm (on_ok),
        // not as the inputs' PressEnter, so saving hangs off on_ok alone.

        this.reload(window, cx);
        this
    }

    /// Runs one repo operation behind the lock, returning an owned result
    /// so no guard borrow outlives the call.
    fn with_repo<T>(
        &self,
        f: impl FnOnce(&Repo) -> kaseer_core::repos::Result<T>,
    ) -> Option<kaseer_core::repos::Result<T>> {
        let guard = self.repo.as_ref()?.lock().ok()?;
        Some(f(&guard))
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.with_repo(|repo| repo.customers(None)) {
            Some(Ok(customers)) => self.customers = customers,
            Some(Err(e)) => Self::toast_error(window, format!("Failed to load customers: {e}"), cx),
            None => {}
        }
        cx.notify();
    }

    fn filtered(&self) -> Vec<&Customer> {
        self.customers
            .iter()
            .filter(|c| {
                self.filter.is_empty()
                    || c.name.to_lowercase().contains(&self.filter)
                    || c.phone.to_lowercase().contains(&self.filter)
            })
            .collect()
    }

    fn adjust_points(&mut self, id: &str, delta: i64, window: &mut Window, cx: &mut Context<Self>) {
        let result = self.with_repo(|repo| {
            repo.adjust_customer_points(id, delta)
                .and_then(|()| repo.customer_by_id(id).map(|_| ()))
        });
        match result {
            Some(Ok(())) => {
                Self::toast_ok(
                    window,
                    if delta > 0 {
                        "Points added"
                    } else {
                        "Points removed"
                    },
                    cx,
                );
                self.reload(window, cx);
            }
            Some(Err(e)) => Self::toast_error(window, format!("Failed to adjust points: {e}"), cx),
            None => {}
        }
    }

    fn sell_to(&mut self, customer: Customer, window: &mut Window, cx: &mut Context<Self>) {
        cx.set_global(SelectedCustomer(Some(customer.clone())));
        Self::toast_ok(window, format!("Selling to {}", customer.name), cx);
    }

    /// Saves the add-dialog's customer; true when saved (the dialog then
    /// closes), false to keep the dialog open after a failure.
    fn save_customer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let name = self.name_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            Self::toast_error(window, "Name is required", cx);
            return false;
        }
        let phone = self.phone_input.read(cx).value().trim().to_string();
        let customer = Customer {
            id: new_customer_id(),
            name: name.clone(),
            phone,
            points: 0,
            created_at: now_iso(),
        };
        let result = self.with_repo(|repo| repo.upsert_customer(&customer));
        match result {
            Some(Ok(_)) => {
                for input in [self.name_input.clone(), self.phone_input.clone()] {
                    input.update(cx, |state, cx| state.set_value("", window, cx));
                }
                Self::toast_ok(window, format!("Added {name}"), cx);
                self.reload(window, cx);
                true
            }
            Some(Err(e)) => {
                Self::toast_error(window, format!("Failed to add customer: {e}"), cx);
                false
            }
            None => {
                Self::toast_error(window, "Database unavailable", cx);
                false
            }
        }
    }

    fn open_add_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [self.name_input.clone(), self.phone_input.clone()] {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        let this = cx.entity().downgrade();
        let name = self.name_input.clone();
        let phone = self.phone_input.clone();
        let theme = cx.theme();
        let surface = theme.popover;
        let field_bg = theme.background;
        let muted = theme.muted_foreground;
        // Type the name straight away.
        let focus = self.name_input.clone();
        window.defer(cx, move |window, cx| {
            focus.update(cx, |state, cx| state.focus(window, cx));
        });
        window.open_dialog(cx, move |dialog, _, _| {
            let this = this.clone();
            let field = |label: &'static str, input: &Entity<InputState>, id: &'static str| {
                v_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(touch::FONT_LABEL)
                            .text_color(muted)
                            .child(label),
                    )
                    .child(
                        h_flex()
                            .h(touch::TAP)
                            .px(px(16.))
                            .items_center()
                            .rounded(touch::RADIUS)
                            .bg(field_bg)
                            .border_1()
                            .border_color(crate::theme::border_strong())
                            .child(
                                Input::new(input)
                                    .id(id)
                                    .appearance(false)
                                    .text_size(px(18.)),
                            ),
                    )
            };
            dialog
                .title("Add customer")
                .w(px(560.))
                .bg(surface)
                .child(
                    v_flex()
                        .gap(px(16.))
                        .child(field("Name", &name, "customer-name"))
                        .child(field("Phone", &phone, "customer-phone")),
                )
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().trigger(|button| {
                            button
                                .child(button_text("Cancel"))
                                .ghost()
                                .large()
                                .h(touch::TAP)
                                .px(px(24.))
                        }))
                        .child(
                            DialogAction::new().child(
                                Button::new("customer-save")
                                    .child(button_text("Save customer"))
                                    .primary()
                                    .large()
                                    .h(touch::TAP)
                                    .px(px(32.)),
                            ),
                        ),
                )
                // The Save button and Enter both land here; a failed
                // validation keeps the dialog open.
                .on_ok(move |_, window, cx| {
                    this.upgrade()
                        .map(|this| this.update(cx, |this, cx| this.save_customer(window, cx)))
                        .unwrap_or(false)
                })
        });
    }

    fn toast_ok(window: &mut Window, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        window.push_notification(Notification::success(message), cx);
    }

    fn toast_error(window: &mut Window, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        window.push_notification(Notification::error(message), cx);
    }
}

/// Fresh customer id. A local register never needs a central id authority,
/// so a monotonic wall-clock hex id stands in for the web's
/// `crypto.randomUUID()`.
fn new_customer_id() -> String {
    format!("c-{:032x}", now_unix_ms())
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// RFC 3339 UTC stamp, the shape the ledger and ISO-range queries expect.
fn now_iso() -> String {
    ms_to_iso(now_unix_ms())
}

impl Render for CustomersScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .v_flex()
            .p(px(16.))
            .gap(px(14.))
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_toolbar(cx))
            .when_some(self.load_error.clone(), |screen, error| {
                screen.child(
                    div()
                        .text_size(touch::FONT_BODY)
                        .text_color(cx.theme().danger)
                        .child(SharedString::from(error)),
                )
            })
            .child(self.render_list(cx))
    }
}

impl CustomersScreen {
    /// Search bar (the register's scan bar in shape), Refresh, Add customer.
    fn render_toolbar(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .w_full()
            .gap(px(12.))
            .items_center()
            .child(
                // The input's editor renders size_full; the fixed-height
                // frame pins it.
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h(px(60.))
                    .px(px(18.))
                    .gap(px(14.))
                    .items_center()
                    .overflow_hidden()
                    .rounded(touch::RADIUS)
                    .bg(theme.background)
                    .border_1()
                    .border_color(crate::theme::border_strong())
                    .child(
                        Icon::new(IconName::Search)
                            .with_size(px(22.))
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        Input::new(&self.search)
                            .id("customer-search")
                            .appearance(false)
                            .text_size(px(17.))
                            .flex_1(),
                    ),
            )
            .child(
                div()
                    .id("refresh-customers")
                    .cursor_pointer()
                    .size(px(60.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(touch::RADIUS)
                    .bg(theme.secondary)
                    .border_1()
                    .border_color(crate::theme::border_strong())
                    .hover(|style| style.opacity(0.85))
                    .active(|style| style.opacity(0.7))
                    .child(
                        Icon::new(IconName::RotateCw)
                            .with_size(px(22.))
                            .text_color(theme.muted_foreground),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
            )
            .child(
                Button::new("open-add-customer")
                    .child(button_text("Add customer"))
                    .icon(IconName::Plus)
                    .primary()
                    .h(px(60.))
                    .px(px(24.))
                    .large()
                    .on_click(cx.listener(|this, _, window, cx| this.open_add_dialog(window, cx))),
            )
    }

    fn render_list(&self, cx: &Context<Self>) -> impl IntoElement {
        let filtered = self.filtered();
        let count = filtered.len();
        let body = if filtered.is_empty() {
            let message = if self.filter.is_empty() {
                "No customers yet. Add your first regular."
            } else {
                "No customers match that search."
            };
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .py(px(40.))
                .text_size(touch::FONT_BODY)
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(message))
                .into_any_element()
        } else {
            v_flex()
                .children(filtered.into_iter().map(|c| self.render_row(cx, c)))
                .into_any_element()
        };

        super::panel(cx)
            .flex_1()
            .min_h_0()
            .gap(px(4.))
            .child(
                h_flex()
                    .items_center()
                    .pb(px(8.))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(super::section_title(cx, format!("Customers: {count}"))),
            )
            .child(
                div()
                    .id("customer-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(body),
            )
    }

    fn render_row(&self, cx: &Context<Self>, customer: &Customer) -> impl IntoElement {
        let theme = cx.theme();
        let hover = theme.secondary_hover;
        let dec_target = customer.id.clone();
        let inc_target = customer.id.clone();
        let sell_customer = customer.clone();
        let step = |id: String, icon: IconName| {
            div()
                .id(SharedString::from(id))
                .cursor_pointer()
                .size(touch::TAP_SM)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(10.))
                .bg(theme.secondary)
                .border_1()
                .border_color(crate::theme::border_strong())
                .hover(move |style| style.bg(hover))
                .active(|style| style.opacity(0.7))
                .child(Icon::new(icon).with_size(px(20.)))
        };
        let phone = if customer.phone.is_empty() {
            "No phone".to_string()
        } else {
            customer.phone.clone()
        };

        h_flex()
            .min_h(px(72.))
            .items_center()
            .gap(px(16.))
            .px(px(10.))
            .py(px(8.))
            .border_b_1()
            .border_color(theme.border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(touch::FONT_TILE)
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(SharedString::from(customer.name.clone())),
                    )
                    .child(
                        h_flex()
                            .gap(px(12.))
                            .text_size(touch::FONT_LABEL)
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(phone))
                            .child(
                                div()
                                    .font_family(crate::theme::FONT_MONO)
                                    .text_size(px(13.))
                                    .text_color(crate::theme::text_dim())
                                    .child(SharedString::from(format!(
                                        "since {}",
                                        created_day(&customer.created_at)
                                    ))),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(8.))
                    .items_center()
                    .child(
                        step(format!("dec-points-{}", customer.id), IconName::Minus).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.adjust_points(&dec_target, -POINTS_STEP, window, cx)
                            }),
                        ),
                    )
                    .child(
                        super::money_text(customer.points.to_string())
                            .w(px(64.))
                            .text_center()
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        step(format!("inc-points-{}", customer.id), IconName::Plus).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.adjust_points(&inc_target, POINTS_STEP, window, cx)
                            }),
                        ),
                    ),
            )
            .child(
                Button::new(SharedString::from(format!("sell-to-{}", customer.id)))
                    .child(button_text("Sell to"))
                    .icon(IconName::ShoppingCart)
                    .secondary()
                    .h(touch::TAP_SM)
                    .px(px(22.))
                    .large()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.sell_to(sell_customer.clone(), window, cx)
                    })),
            )
    }
}

/// The UTC calendar day of an RFC 3339 stamp: the web's
/// `createdAt.toLocaleDateString()` display contract.
fn created_day(created_at: &str) -> String {
    created_at.get(..10).unwrap_or(created_at).to_string()
}

/// A touch-size button label (Button sizes its own label text by variant).
fn button_text(text: &'static str) -> gpui_kit::Div {
    div()
        .text_size(px(17.))
        .font_weight(FontWeight::MEDIUM)
        .child(text)
}
