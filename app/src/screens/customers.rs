//! Customers screen: search, sell-to, points adjust, and the add-customer
//! dialog, ported 1:1 from the web `customers.tsx`.

use std::sync::Mutex;

use gpui_kit::base::{StyledExt as _, h_flex, v_flex};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, button::Button, button::ButtonVariants as _,
    form::Field, input::Input, input::InputEvent, input::InputState,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, Global, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Window, div, px,
};
use kaseer_core::db::{open_repo, resolve_db_path};
use kaseer_core::reports::ms_to_iso;
use kaseer_core::repos::{Customer, Repo};

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

/// Points moved by one ± click, matching the web screen.
const POINTS_STEP: i64 = 10;

/// The customers panel: search, table, add-customer dialog.
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
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Name"));
        let phone_input = cx.new(|cx| InputState::new(window, cx).placeholder("Phone"));

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
        for input in [this.name_input.clone(), this.phone_input.clone()] {
            this._subscriptions.push(cx.subscribe_in(
                &input,
                window,
                |this, _, event, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.save_customer(window, cx);
                    }
                },
            ));
        }

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

    fn save_customer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            Self::toast_error(window, "Name is required", cx);
            return;
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
                window.close_dialog(cx);
                for input in [self.name_input.clone(), self.phone_input.clone()] {
                    input.update(cx, |state, cx| state.set_value("", window, cx));
                }
                Self::toast_ok(window, format!("Added {name}"), cx);
                self.reload(window, cx);
            }
            Some(Err(e)) => Self::toast_error(window, format!("Failed to add customer: {e}"), cx),
            None => Self::toast_error(window, "Database unavailable", cx),
        }
    }

    fn open_add_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [self.name_input.clone(), self.phone_input.clone()] {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        let this = cx.entity().downgrade();
        let name = self.name_input.clone();
        let phone = self.phone_input.clone();
        let surface = cx.theme().popover;
        window.open_dialog(cx, move |dialog, _, _| {
            let this = this.clone();
            dialog
                .title("Add customer")
                .bg(surface)
                .child(
                    v_flex()
                        .gap_3()
                        .w_96()
                        .child(Field::new().label("Name").child(Input::new(&name)))
                        .child(Field::new().label("Phone").child(Input::new(&phone))),
                )
                .when_some(this.upgrade(), |dialog, this| {
                    dialog.footer(
                        h_flex().justify_end().gap_2().child(
                            Button::new("customer-save")
                                .label("Save customer")
                                .primary()
                                .on_click(move |_, window, cx| {
                                    // A failed validation keeps the dialog
                                    // open; success closes it from save.
                                    this.update(cx, |this, cx| this.save_customer(window, cx));
                                }),
                        ),
                    )
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
            .p_4()
            .gap_3()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_panel(cx))
    }
}

impl CustomersScreen {
    fn render_panel(&self, cx: &Context<Self>) -> impl IntoElement {
        let filtered = self.filtered();

        v_flex()
            .flex_1()
            .min_h_0()
            .gap_3()
            .p(px(14.))
            .rounded(crate::theme::CARD_RADIUS)
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .flex_wrap()
                    .gap_2()
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(
                        SharedString::from(format!("Customers: {}", filtered.len())),
                    ))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().w_56().child(Input::new(&self.search)))
                            .child(
                                Button::new("refresh-customers")
                                    .label("Refresh")
                                    .icon(IconName::RotateCw)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.reload(window, cx)),
                                    ),
                            )
                            .child(
                                Button::new("open-add-customer")
                                    .label("Add customer")
                                    .icon(IconName::Plus)
                                    .primary()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_add_dialog(window, cx)
                                    })),
                            ),
                    ),
            )
            .when_some(self.load_error.clone(), |panel, error| {
                panel.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(SharedString::from(error)),
                )
            })
            .child(self.render_table(cx, filtered))
    }

    fn render_table(&self, cx: &Context<Self>, filtered: Vec<&Customer>) -> impl IntoElement {
        if filtered.is_empty() {
            let message = if self.filter.is_empty() {
                "No customers yet. Add your first regular."
            } else {
                "No customers match that search."
            };
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(message))
                .into_any_element();
        }

        let rows = filtered
            .into_iter()
            .map(|c| {
                let customer = c.clone();
                let dec_id = format!("dec-points-{}", customer.id);
                let dec_target = customer.id.clone();
                let inc_id = format!("inc-points-{}", customer.id);
                let inc_target = customer.id.clone();
                let sell_id = format!("sell-to-{}", customer.id);
                let sell_customer = customer.clone();
                let row = TableRow::new()
                    .child(
                        TableCell::new()
                            .font_medium()
                            .child(SharedString::from(customer.name.clone())),
                    )
                    .child(
                        TableCell::new()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(customer.phone.clone())),
                    )
                    .child(
                        TableCell::new().child(
                            h_flex()
                                .gap_1()
                                .child(Button::new(dec_id).xsmall().icon(IconName::Minus).on_click(
                                    cx.listener(move |this, _, window, cx| {
                                        this.adjust_points(&dec_target, -POINTS_STEP, window, cx)
                                    }),
                                ))
                                .child(
                                    div()
                                        .min_w_12()
                                        .text_center()
                                        .font_medium()
                                        .font_features(crate::theme::tabular())
                                        .child(SharedString::from(customer.points.to_string())),
                                )
                                .child(Button::new(inc_id).xsmall().icon(IconName::Plus).on_click(
                                    cx.listener(move |this, _, window, cx| {
                                        this.adjust_points(&inc_target, POINTS_STEP, window, cx)
                                    }),
                                )),
                        ),
                    )
                    .child(
                        TableCell::new()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(created_day(&customer.created_at))),
                    );
                row.child(
                    TableCell::new().child(
                        Button::new(sell_id)
                            .xsmall()
                            .primary()
                            .label("Sell to")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.sell_to(sell_customer.clone(), window, cx)
                            })),
                    ),
                )
            })
            .collect::<Vec<_>>();

        div()
            .min_w_0()
            .overflow_x_scrollbar()
            .child(
                Table::new()
                    .accessibility_label("Customers")
                    .child(
                        TableHeader::new().child(
                            TableRow::new()
                                .child(TableHead::new().text_size(px(11.)).child("NAME"))
                                .child(TableHead::new().text_size(px(11.)).child("PHONE"))
                                .child(TableHead::new().text_size(px(11.)).child("POINTS"))
                                .child(TableHead::new().text_size(px(11.)).child("CREATED"))
                                .child(TableHead::new()),
                        ),
                    )
                    .child(TableBody::new().children(rows)),
            )
            .into_any_element()
    }
}

/// The UTC calendar day of an RFC 3339 stamp: the web's
/// `createdAt.toLocaleDateString()` display contract.
fn created_day(created_at: &str) -> String {
    created_at.get(..10).unwrap_or(created_at).to_string()
}
