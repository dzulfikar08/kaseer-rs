//! Settings screen: store profile, loyalty program, retention window, and
//! the tax-rates table, ported 1:1 from the web `settings.tsx`. Profile and
//! loyalty numbers save through the Save button; tax percents save when a
//! field loses focus.

use std::sync::Mutex;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Sizable as _, button::Button,
    button::ButtonVariants as _, form::Field, input::Input, input::InputEvent, input::InputState,
    searchable_list::SearchableListItem, searchable_list::SearchableVec, select::Select,
    select::SelectEvent, select::SelectState, switch::Switch,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EntityId, FontWeight, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Window, div, px, rems,
};
use kaseer_core::currency::{CURRENCIES, currency_by_code};
use kaseer_core::db::{open_repo, resolve_db_path};
use kaseer_core::loyalty::LoyaltyConfig;
use kaseer_core::money as money_core;
use kaseer_core::repos::{Repo, Settings, StoreConfig};
use kaseer_core::tax::TaxRate;

/// One editable percent cell: the stored rate plus its input.
struct TaxField {
    rate: TaxRate,
    pct: Entity<InputState>,
}

/// One currency option in the dropdown; the select's value is the code.
#[derive(Clone)]
struct CurrencyItem {
    code: SharedString,
    label: SharedString,
}

impl SearchableListItem for CurrencyItem {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.code
    }
}

/// The settings column: store profile, loyalty, save row, tax rates.
pub struct SettingsScreen {
    /// `None` when the database could not be opened.
    repo: Option<Mutex<Repo>>,
    store_name: Entity<InputState>,
    address: Entity<InputState>,
    receipt_footer: Entity<InputState>,
    currency: Entity<SelectState<SearchableVec<CurrencyItem>>>,
    currency_code: String,
    loyalty_enabled: bool,
    step_minor: Entity<InputState>,
    points_per_step: Entity<InputState>,
    point_value_minor: Entity<InputState>,
    lost_after_days: Entity<InputState>,
    tax_fields: Vec<TaxField>,
    load_error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (repo, settings, tax_rates, load_error) = match open_repo(&resolve_db_path()) {
            Ok(repo) => match (repo.settings(), repo.tax_rates()) {
                (Ok(settings), Ok(tax_rates)) => (Some(Mutex::new(repo)), Some(settings), tax_rates, None),
                (Err(e), _) | (_, Err(e)) => (None, None, Vec::new(), Some(e.to_string())),
            },
            Err(e) => (None, None, Vec::new(), Some(e.to_string())),
        };
        let store = settings.as_ref();

        let store_name = text_input(window, cx, store.map_or("", |s| &s.store_name), "Toko Kaseer");
        let address = text_input(window, cx, store.map_or("", |s| &s.address), "");
        let receipt_footer = text_input(window, cx, store.map_or("", |s| &s.receipt_footer), "e.g. Terima kasih!");
        let step_minor = text_input(window, cx, &number_or_empty(store.map(|s| s.loyalty.step_minor)), "");
        let points_per_step = text_input(window, cx, &number_or_empty(store.map(|s| s.loyalty.points_per_step)), "");
        let point_value_minor = text_input(window, cx, &number_or_empty(store.map(|s| s.loyalty.point_value_minor)), "");
        let lost_after_days = text_input(window, cx, &number_or_empty(store.map(|s| s.config.lost_after_days)), "");

        let currency_code = store
            .map(|s| s.currency_code.clone())
            .unwrap_or_else(|| kaseer_core::currency::IDR.code.to_string());
        let items = CURRENCIES
            .iter()
            .map(|c| CurrencyItem {
                code: SharedString::from(c.code),
                label: SharedString::from(format!("{} · {}", c.code, c.symbol)),
            })
            .collect::<Vec<_>>();
        let selected = CURRENCIES
            .iter()
            .position(|c| c.code == currency_code)
            .map(IndexPath::new);
        let currency = cx.new(|cx| SelectState::new(SearchableVec::new(items), selected, window, cx));

        let mut tax_fields = Vec::new();
        for rate in &tax_rates {
            let pct = text_input(window, cx, &bps_to_percent(rate.bps), "");
            tax_fields.push(TaxField { rate: rate.clone(), pct });
        }

        let mut this = Self {
            repo,
            store_name,
            address,
            receipt_footer,
            currency,
            currency_code,
            loyalty_enabled: store.is_some_and(|s| s.loyalty.enabled),
            step_minor,
            points_per_step,
            point_value_minor,
            lost_after_days,
            tax_fields,
            load_error,
            _subscriptions: Vec::new(),
        };

        this._subscriptions.push(cx.subscribe_in(
            &this.currency.clone(),
            window,
            |this, _, event: &SelectEvent<SearchableVec<CurrencyItem>>, _, cx| {
                if let SelectEvent::Confirm(Some(code)) = event {
                    this.currency_code = code.to_string();
                    cx.notify();
                }
            },
        ));
        for field in &this.tax_fields {
            let pct = field.pct.clone();
            this._subscriptions.push(cx.subscribe_in(
                &pct,
                window,
                |this, state, event, window, cx| {
                    if matches!(event, InputEvent::Blur) {
                        this.save_tax_rate(state.entity_id(), window, cx);
                    }
                },
            ));
        }

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

    fn save_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = Settings {
            store_name: self.store_name.read(cx).value().trim().to_string(),
            address: self.address.read(cx).value().trim().to_string(),
            receipt_footer: self.receipt_footer.read(cx).value().trim().to_string(),
            currency_code: self.currency_code.clone(),
            loyalty: LoyaltyConfig {
                enabled: self.loyalty_enabled,
                step_minor: non_neg_int(&self.step_minor.read(cx).value()),
                points_per_step: non_neg_int(&self.points_per_step.read(cx).value()),
                point_value_minor: non_neg_int(&self.point_value_minor.read(cx).value()),
            },
            config: StoreConfig {
                lost_after_days: non_neg_int(&self.lost_after_days.read(cx).value()),
            },
        };
        match self.with_repo(|repo| repo.update_settings(&next)) {
            Some(Ok(())) => Self::toast_ok(window, "Settings saved", cx),
            Some(Err(_)) => Self::toast_error(window, "Could not save settings", cx),
            None => Self::toast_error(window, "Database unavailable", cx),
        }
    }

    /// Blur-commit for one percent cell, matched by the input's entity id.
    fn save_tax_rate(&mut self, input_id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.tax_fields.iter().position(|f| f.pct.entity_id() == input_id) else {
            return;
        };
        let value = self.tax_fields[ix].pct.read(cx).value().to_string();
        let bps = percent_to_bps(&value);
        if bps == self.tax_fields[ix].rate.bps {
            return;
        }
        let mut rate = self.tax_fields[ix].rate.clone();
        rate.bps = bps;
        let name = rate.name.clone();
        match self.with_repo(|repo| repo.upsert_tax_rate(&rate)) {
            Some(Ok(())) => {
                self.tax_fields[ix].rate.bps = bps;
                Self::toast_ok(window, format!("{name} updated to {}%", bps_to_percent(bps)), cx);
            }
            Some(Err(_)) => Self::toast_error(window, format!("Could not update {name}"), cx),
            None => {}
        }
    }

    fn toast_ok(window: &mut Window, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        window.push_notification(Notification::success(message), cx);
    }

    fn toast_error(window: &mut Window, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        window.push_notification(Notification::error(message), cx);
    }
}

/// A text input seeded with `initial`, the constructor's only nontrivial
/// helper (input state cannot be built without a live context).
fn text_input(
    window: &mut Window,
    cx: &mut Context<SettingsScreen>,
    initial: &str,
    placeholder: &'static str,
) -> Entity<InputState> {
    cx.new(|cx| {
        let mut state = InputState::new(window, cx).placeholder(placeholder);
        state.set_value(initial, window, cx);
        state
    })
}

/// The stored number as text, or "" when there is no settings row.
fn number_or_empty(value: Option<i64>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

/// The web form's `num`: clamp negatives to zero, round, never NaN-trap.
fn non_neg_int(value: &str) -> i64 {
    let n: f64 = value.trim().parse().unwrap_or(0.0);
    if n.is_finite() { n.max(0.0).round() as i64 } else { 0 }
}

/// Basis points to a plain percent string: 1_100 → "11", 1_050 → "10.5".
fn bps_to_percent(bps: i64) -> String {
    format!("{}", bps as f64 / 100.)
}

/// The web's `Math.round((Number(raw) || 0) * 100)`.
fn percent_to_bps(value: &str) -> i64 {
    let n: f64 = value.trim().parse().unwrap_or(0.0);
    if n.is_finite() { (n * 100.).round() as i64 } else { 0 }
}

/// Money as the store's currency prints it, for the loyalty hint line.
fn money_text(minor: i64, code: &str) -> String {
    match currency_by_code(code) {
        Ok(cur) => money_core::format(&money_core::money(minor, code), Some(cur))
            .unwrap_or_else(|_| minor.to_string()),
        Err(_) => minor.to_string(),
    }
}

impl Render for SettingsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .overflow_y_scrollbar()
            .flex()
            .justify_center()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                v_flex()
                    .w_full()
                    .max_w(rems(60.))
                    .p_4()
                    .gap_3()
                    .child(self.render_profile_row(cx))
                    .child(self.render_save_row(cx))
                    .child(self.render_tax_panel(cx)),
            )
    }
}

impl SettingsScreen {
    fn render_profile_row(&self, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .items_stretch()
            .flex_wrap()
            .gap_3()
            .child(self.render_store_panel(cx))
            .child(self.render_loyalty_panel(cx))
    }

    fn render_store_panel(&self, cx: &Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_1()
            .min_w_80()
            .gap_3()
            .p(px(14.))
            .rounded(crate::theme::CARD_RADIUS)
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child("STORE"),
            )
            .when_some(self.load_error.clone(), |panel, error| {
                panel.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(SharedString::from(error)),
                )
            })
            .child(Field::new().label("Store name").child(Input::new(&self.store_name)))
            .child(
                Field::new()
                    .label("Address (printed on receipts)")
                    .child(Input::new(&self.address)),
            )
            .child(
                Field::new()
                    .label("Receipt footer")
                    .child(Input::new(&self.receipt_footer)),
            )
            .child(Field::new().label("Currency").child(Select::new(&self.currency)))
    }

    fn render_loyalty_panel(&self, cx: &Context<Self>) -> impl IntoElement {
        let currency = self.currency_code.clone();
        let step_minor = non_neg_int(&self.step_minor.read(cx).value());
        let points = non_neg_int(&self.points_per_step.read(cx).value());
        let point_value = non_neg_int(&self.point_value_minor.read(cx).value());

        v_flex()
            .flex_1()
            .min_w_80()
            .gap_3()
            .p(px(14.))
            .rounded(crate::theme::CARD_RADIUS)
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child("LOYALTY"),
            )
            .child(
                Switch::new("loyalty-enabled")
                    .label("Enable loyalty points")
                    .checked(self.loyalty_enabled)
                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                        this.loyalty_enabled = *checked;
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_24()
                            .gap_1()
                            .child(
                                div().text_xs().text_color(cx.theme().muted_foreground).child(
                                    SharedString::from(format!("Spend per step ({})", self.currency_code)),
                                ),
                            )
                            .child(Input::new(&self.step_minor).small()),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_24()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from("Points per step")),
                            )
                            .child(Input::new(&self.points_per_step).small()),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_24()
                            .gap_1()
                            .child(
                                div().text_xs().text_color(cx.theme().muted_foreground).child(
                                    SharedString::from(format!("Point value ({})", self.currency_code)),
                                ),
                            )
                            .child(Input::new(&self.point_value_minor).small()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(format!(
                        "{} of spend earns {} pts · 1 pt = {} off.",
                        money_text(step_minor, &currency),
                        points,
                        money_text(point_value, &currency),
                    ))),
            )
            .child(
                v_flex()
                    .gap_1()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .pt_2()
                    .child(
                        div().text_xs().text_color(cx.theme().muted_foreground).child(
                            SharedString::from("Customer considered lost after (days without purchase)"),
                        ),
                    )
                    .child(div().w_24().child(Input::new(&self.lost_after_days).small())),
            )
    }

    fn render_save_row(&self, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .justify_between()
            .flex_wrap()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(
                        "Currency changes reprice every screen after save.",
                    )),
            )
            .child(
                Button::new("save-settings")
                    .label("Save settings")
                    .primary()
                    .disabled(self.repo.is_none())
                    .on_click(cx.listener(|this, _, window, cx| this.save_settings(window, cx))),
            )
    }

    fn render_tax_panel(&self, cx: &Context<Self>) -> impl IntoElement {
        let rows = self
            .tax_fields
            .iter()
            .map(|field| {
                TableRow::new()
                    .child(TableCell::new().child(SharedString::from(field.rate.name.clone())))
                    .child(TableCell::new().child(div().w_24().child(Input::new(&field.pct).small())))
                    .child(
                        TableCell::new()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(if field.rate.inclusive {
                                "Inclusive"
                            } else {
                                "Exclusive"
                            })),
                    )
            })
            .collect::<Vec<_>>();

        v_flex()
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
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().muted_foreground)
                            .child("TAX RATES"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(
                                "Percent — saved when a field loses focus",
                            )),
                    ),
            )
            .child(self.render_tax_table(cx, rows))
    }

    fn render_tax_table(&self, cx: &Context<Self>, rows: Vec<TableRow>) -> impl IntoElement {
        let rates_ok = self
            .with_repo(|repo| repo.tax_rates().map(|rates| !rates.is_empty()));
        match rates_ok {
            Some(Ok(true)) => Table::new()
                .accessibility_label("Tax rates")
                .child(
                    TableHeader::new().child(
                        TableRow::new()
                            .child(TableHead::new().text_size(px(11.)).child("NAME"))
                            .child(TableHead::new().text_size(px(11.)).child("RATE %"))
                            .child(TableHead::new().text_size(px(11.)).child("TYPE")),
                    ),
                )
                .child(TableBody::new().children(rows))
                .into_any_element(),
            Some(Ok(false)) => div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from("No tax rates configured."))
                .into_any_element(),
            Some(Err(e)) => div()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(SharedString::from(e.to_string()))
                .into_any_element(),
            None => div().into_any_element(),
        }
    }
}
