//! Settings screen: store profile, loyalty program, retention window, and
//! the tax-rates table, ported 1:1 from the web `settings.tsx`. Profile and
//! loyalty numbers save through the Save button; tax percents save when a
//! field loses focus.

use std::sync::Mutex;
use std::time::Duration;

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Sizable as _, button::Button,
    button::ButtonVariants as _, input::Input, input::InputEvent, input::InputState,
    searchable_list::SearchableListItem, searchable_list::SearchableVec, select::Select,
    select::SelectEvent, select::SelectState, switch::Switch,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Div, Entity, EntityId, FontWeight, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Window, div, px, rems,
};
use kaseer_core::currency::{CURRENCIES, currency_by_code};
use kaseer_core::db::{open_repo, resolve_db_path};
use kaseer_core::loyalty::LoyaltyConfig;
use kaseer_core::money as money_core;
use kaseer_core::repos::{Repo, Settings, StoreConfig};
use kaseer_core::sync::{self, Outbox};
use kaseer_core::tax::TaxRate;

use crate::theme::metrics;

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
    /// Grow switches (StoreConfig); Kaseer Cloud overwrites them on a menu pull.
    addon_prompt: bool,
    ask_phone: bool,
    winback_discount_pct: i64,
    cloud_addons: Vec<kaseer_core::repos::CloudAddon>,
    step_minor: Entity<InputState>,
    points_per_step: Entity<InputState>,
    point_value_minor: Entity<InputState>,
    lost_after_days: Entity<InputState>,
    tax_fields: Vec<TaxField>,
    load_error: Option<String>,
    _subscriptions: Vec<Subscription>,
    /// Cloud sync: config lives in the outbox's own SQLite file (device
    /// local), never in the shared ledger.
    cloud_url: Entity<InputState>,
    cloud_token: Entity<InputState>,
    outbox: Option<Outbox>,
    sync_in_flight: bool,
    sync_message: Option<String>,
    /// Menu pull: companion endpoint (same outbox meta table), status line,
    /// and the manual pull's in-flight flag.
    menu_url: Entity<InputState>,
    menu_in_flight: bool,
    menu_message: Option<String>,
    /// Cloud kiosk: enrollment lives in the same outbox meta table. The
    /// pairing code is one-shot and never persisted; the device token the
    /// cloud returns is write-only (never rendered again).
    enroll_url: Entity<InputState>,
    pairing_code: Entity<InputState>,
    enroll_in_flight: bool,
    enroll_message: Option<String>,
}

impl SettingsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (repo, settings, tax_rates, load_error) = match open_repo(&resolve_db_path()) {
            Ok(repo) => match (repo.settings(), repo.tax_rates()) {
                (Ok(settings), Ok(tax_rates)) => {
                    (Some(Mutex::new(repo)), Some(settings), tax_rates, None)
                }
                (Err(e), _) | (_, Err(e)) => (None, None, Vec::new(), Some(e.to_string())),
            },
            Err(e) => (None, None, Vec::new(), Some(e.to_string())),
        };
        let store = settings.as_ref();

        let store_name = text_input(
            window,
            cx,
            store.map_or("", |s| &s.store_name),
            "Toko Kaseer",
        );
        let address = text_input(window, cx, store.map_or("", |s| &s.address), "");
        let receipt_footer = text_input(
            window,
            cx,
            store.map_or("", |s| &s.receipt_footer),
            "e.g. Terima kasih!",
        );
        let step_minor = text_input(
            window,
            cx,
            &number_or_empty(store.map(|s| s.loyalty.step_minor)),
            "",
        );
        let points_per_step = text_input(
            window,
            cx,
            &number_or_empty(store.map(|s| s.loyalty.points_per_step)),
            "",
        );
        let point_value_minor = text_input(
            window,
            cx,
            &number_or_empty(store.map(|s| s.loyalty.point_value_minor)),
            "",
        );
        let lost_after_days = text_input(
            window,
            cx,
            &number_or_empty(store.map(|s| s.config.lost_after_days)),
            "",
        );

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
        let currency =
            cx.new(|cx| SelectState::new(SearchableVec::new(items), selected, window, cx));

        let mut tax_fields = Vec::new();
        for rate in &tax_rates {
            let pct = text_input(window, cx, &bps_to_percent(rate.bps), "");
            tax_fields.push(TaxField {
                rate: rate.clone(),
                pct,
            });
        }

        let outbox = Outbox::open(&sync::resolve_outbox_path()).ok();
        let cloud_url_value = outbox
            .as_ref()
            .and_then(|o| o.meta_get("cloud_url"))
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(sync::default_cloud_url);
        let cloud_token_value = outbox
            .as_ref()
            .and_then(|o| o.meta_get("store_token"))
            .unwrap_or_default();
        let cloud_url = text_input(window, cx, &cloud_url_value, sync::DEFAULT_CLOUD_URL);
        let cloud_token = text_input(window, cx, &cloud_token_value, "Store token");
        let menu_url_value = outbox
            .as_ref()
            .and_then(|o| o.meta_get("companion_url"))
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(crate::menu_sync::default_companion_url);
        let menu_url = text_input(
            window,
            cx,
            &menu_url_value,
            crate::menu_sync::DEFAULT_COMPANION_URL,
        );
        let pairing_code = text_input(window, cx, "", "6-digit code");

        let mut this = Self {
            repo,
            store_name,
            address,
            receipt_footer,
            currency,
            currency_code,
            loyalty_enabled: store.is_some_and(|s| s.loyalty.enabled),
            addon_prompt: store.is_none_or(|s| s.config.addon_prompt),
            ask_phone: store.is_none_or(|s| s.config.ask_phone),
            winback_discount_pct: store.map_or(10, |s| s.config.winback_discount_pct),
            cloud_addons: store.map(|s| s.config.cloud_addons.clone()).unwrap_or_default(),
            step_minor,
            points_per_step,
            point_value_minor,
            lost_after_days,
            tax_fields,
            load_error,
            _subscriptions: Vec::new(),
            cloud_url,
            cloud_token,
            outbox,
            sync_in_flight: false,
            sync_message: None,
            menu_url,
            menu_in_flight: false,
            menu_message: None,
            enroll_url: text_input(
                window,
                cx,
                &crate::kiosk::default_enroll_url(),
                sync::DEFAULT_CLOUD_URL,
            ),
            pairing_code,
            enroll_in_flight: false,
            enroll_message: None,
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
        for input in [this.cloud_url.clone(), this.cloud_token.clone()] {
            this._subscriptions.push(cx.subscribe_in(
                &input,
                window,
                |this, state, event, _window, cx| {
                    if matches!(event, InputEvent::Blur) {
                        this.save_sync_config(state.entity_id(), cx);
                    }
                },
            ));
        }
        this._subscriptions.push(cx.subscribe_in(
            &this.menu_url.clone(),
            window,
            |this, state, event, _window, cx| {
                if matches!(event, InputEvent::Blur) {
                    this.save_menu_config(state.entity_id(), cx);
                }
            },
        ));

        this.spawn_pending_refresh(cx);

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
                addon_prompt: self.addon_prompt,
                ask_phone: self.ask_phone,
                winback_discount_pct: self.winback_discount_pct,
                cloud_addons: self.cloud_addons.clone(),
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
        let Some(ix) = self
            .tax_fields
            .iter()
            .position(|f| f.pct.entity_id() == input_id)
        else {
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
                Self::toast_ok(
                    window,
                    format!("{name} updated to {}%", bps_to_percent(bps)),
                    cx,
                );
            }
            Some(Err(_)) => Self::toast_error(window, format!("Could not update {name}"), cx),
            None => {}
        }
    }

    /// Blur-commit for the cloud URL and store token, persisted in the
    /// outbox's meta table. Clearing a field resets it to its default
    /// (the meta key is dropped), matching the seed shown on next visit.
    fn save_sync_config(&mut self, input_id: EntityId, cx: &mut Context<Self>) {
        let Some(outbox) = &self.outbox else {
            return;
        };
        let (key, value) = if input_id == self.cloud_url.entity_id() {
            (
                "cloud_url",
                self.cloud_url.read(cx).value().trim().to_string(),
            )
        } else if input_id == self.cloud_token.entity_id() {
            (
                "store_token",
                self.cloud_token.read(cx).value().trim().to_string(),
            )
        } else {
            return;
        };
        if outbox.meta_get(key).as_deref() == Some(value.as_str()) {
            return;
        }
        let saved = if value.is_empty() {
            outbox.meta_delete(key)
        } else {
            outbox.meta_set(key, &value)
        };
        match saved {
            Ok(()) => cx.notify(),
            Err(error) => self.sync_message = Some(format!("Could not save: {error}")),
        }
    }

    /// Keeps the pending count and status line honest: retained screens
    /// only re-render when notified, and sales settle on other screens.
    /// The loop ends when the screen entity is dropped.
    fn spawn_pending_refresh(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// (pending, last sync, last error) straight from the outbox file.
    fn sync_status(&self) -> (u64, Option<String>, Option<String>) {
        let Some(outbox) = &self.outbox else {
            return (0, None, None);
        };
        (
            outbox.pending_count(),
            outbox.meta_get("last_sync_at"),
            outbox.meta_get("last_error"),
        )
    }

    /// Forced drain on the background executor: ignores the failure
    /// backoff window, flips the button into its in-flight state, and
    /// reports through the status line: never a modal or toast.
    fn sync_now(&mut self, cx: &mut Context<Self>) {
        if self.sync_in_flight {
            return;
        }
        self.sync_in_flight = true;
        self.sync_message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { crate::sync_client::drain_once(true) })
                .await;
            this.update(cx, |this, cx| {
                this.sync_in_flight = false;
                this.sync_message = Some(match &result {
                    Ok(outcome) => outcome.summary(),
                    Err(error) => format!("Sync failed: {error}"),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Blur-commit for the companion URL, persisted in the same outbox
    /// meta table as the cloud config. Clearing the field falls back to
    /// `KASEER_COMPANION_URL` (or the loopback default).
    fn save_menu_config(&mut self, input_id: EntityId, cx: &mut Context<Self>) {
        if input_id != self.menu_url.entity_id() {
            return;
        }
        let Some(outbox) = &self.outbox else {
            return;
        };
        let value = self.menu_url.read(cx).value().trim().to_string();
        if outbox.meta_get("companion_url").as_deref() == Some(value.as_str()) {
            return;
        }
        let saved = if value.is_empty() {
            outbox.meta_delete("companion_url")
        } else {
            outbox.meta_set("companion_url", &value)
        };
        match saved {
            Ok(()) => cx.notify(),
            Err(error) => self.menu_message = Some(format!("Could not save: {error}")),
        }
    }

    /// (last pull stamp, last error) straight from the outbox meta.
    fn menu_status(&self) -> (Option<String>, Option<String>) {
        let Some(outbox) = &self.outbox else {
            return (None, None);
        };
        (
            outbox.meta_get("menu_last_sync_at"),
            outbox.meta_get("menu_last_error"),
        )
    }

    /// Manual menu pull on the background executor: ignores the revision
    /// gate, flips the button into its in-flight state, and reports through
    /// the status line: never a modal or toast.
    fn pull_menu_now(&mut self, cx: &mut Context<Self>) {
        if self.menu_in_flight {
            return;
        }
        self.menu_in_flight = true;
        self.menu_message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { crate::menu_sync::pull_now() })
                .await;
            this.update(cx, |this, cx| {
                this.menu_in_flight = false;
                this.menu_message = Some(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The enrolled device identity, read fresh from the outbox meta.
    fn kiosk_identity(&self) -> Option<crate::kiosk::KioskIdentity> {
        self.outbox
            .as_ref()
            .and_then(crate::kiosk::KioskIdentity::from_outbox)
    }

    /// Pairing-code enrollment on the background executor: exchanges the
    /// one-shot code for a device token (stored write-only), then beats a
    /// first heartbeat and pulls the cloud menu so the register switches
    /// sources immediately.
    fn enroll_now(&mut self, cx: &mut Context<Self>) {
        if self.enroll_in_flight {
            return;
        }
        let base_url = self.enroll_url.read(cx).value().trim().to_string();
        let pairing_code = self.pairing_code.read(cx).value().trim().to_string();
        if pairing_code.len() != 6 || !pairing_code.chars().all(|c| c.is_ascii_digit()) {
            self.enroll_message =
                Some("Pairing code is the 6 digits from the cloud console.".to_string());
            cx.notify();
            return;
        }
        self.enroll_in_flight = true;
        self.enroll_message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    crate::kiosk::enroll(&base_url, &pairing_code).inspect(|_| {
                        // First beat + first pull land before the Settings
                        // line even repaints; both are best-effort.
                        let _ = crate::kiosk::heartbeat();
                        crate::menu_sync::poll_once();
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                this.enroll_in_flight = false;
                match result {
                    Ok(identity) => {
                        // The pairing code is one-shot and now dead; it is
                        // left in the field only as a receipt of what was
                        // consumed: never stored anywhere.
                        this.enroll_message = Some(format!("Enrolled as {}.", identity.name));
                    }
                    Err(error) => {
                        this.enroll_message = Some(format!("Enroll failed: {error}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Revocation clears the local identity only: the terminal falls back
    /// to store-token sync and LAN-companion menus; the cloud record stays
    /// for the owner to retire.
    fn revoke_now(&mut self, cx: &mut Context<Self>) {
        match crate::kiosk::revoke() {
            Ok(()) => {
                self.enroll_message =
                    Some("Kiosk revoked: this terminal is no longer enrolled.".to_string());
                crate::menu_sync::poll_once();
            }
            Err(error) => self.enroll_message = Some(format!("Revoke failed: {error}")),
        }
        cx.notify();
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
/// A settings field: the muted 11px caption every panel on this screen
/// uses, over its control. The Store panel used the kit `Field` label
/// (semibold 14px) and stood out from the rest of the page.
fn labeled(cx: &Context<SettingsScreen>, label: &str, control: impl IntoElement) -> Div {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_size(metrics::FONT_FOOTNOTE)
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(label.to_string())),
        )
        .child(control)
}

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
    if n.is_finite() {
        n.max(0.0).round() as i64
    } else {
        0
    }
}

/// Basis points to a plain percent string: 1_100 → "11", 1_050 → "10.5".
fn bps_to_percent(bps: i64) -> String {
    format!("{}", bps as f64 / 100.)
}

/// The web's `Math.round((Number(raw) || 0) * 100)`.
fn percent_to_bps(value: &str) -> i64 {
    let n: f64 = value.trim().parse().unwrap_or(0.0);
    if n.is_finite() {
        (n * 100.).round() as i64
    } else {
        0
    }
}

/// Money as the store's currency prints it, for the loyalty hint line.
fn money_text(minor: i64, code: &str) -> String {
    match currency_by_code(code) {
        Ok(cur) => money_core::format(&money_core::money(minor, code), Some(cur))
            .unwrap_or_else(|_| minor.to_string()),
        Err(_) => minor.to_string(),
    }
}

/// RFC3339 → "YYYY-MM-DD HH:MM" for the sync status line.
fn short_time(iso: &str) -> String {
    iso.get(..16)
        .map(|s| s.replace('T', " "))
        .unwrap_or_else(|| iso.to_string())
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
                    // The kiosk card sits above the owner config: enrollment,
                    // heartbeat, and update state are what a terminal operator
                    // needs at a glance (and the 840px viewport shows it first).
                    .child(self.render_kiosk_panel(cx))
                    .child(self.render_tax_panel(cx))
                    .child(self.render_sync_panel(cx)),
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
        super::panel(cx)
            .flex_1()
            .min_w_80()
            .gap(px(12.))
            .child(
                super::section_title(cx, "STORE"),
            )
            .when_some(self.load_error.clone(), |panel, error| {
                panel.child(
                    div()
                        .text_size(metrics::FONT_FOOTNOTE)
                        .text_color(cx.theme().danger)
                        .child(SharedString::from(error)),
                )
            })
            .child(labeled(cx, "Store name", Input::new(&self.store_name)))
            .child(labeled(
                cx,
                "Address (printed on receipts)",
                Input::new(&self.address),
            ))
            .child(labeled(cx, "Receipt footer", Input::new(&self.receipt_footer)))
            .child(labeled(cx, "Currency", Select::new(&self.currency)))
    }

    fn render_loyalty_panel(&self, cx: &Context<Self>) -> impl IntoElement {
        let currency = self.currency_code.clone();
        let step_minor = non_neg_int(&self.step_minor.read(cx).value());
        let points = non_neg_int(&self.points_per_step.read(cx).value());
        let point_value = non_neg_int(&self.point_value_minor.read(cx).value());

        super::panel(cx)
            .flex_1()
            .min_w_80()
            .gap(px(12.))
            .child(
                super::section_title(cx, "LOYALTY"),
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
                                div()
                                    .text_size(metrics::FONT_FOOTNOTE)
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(format!(
                                        "Spend per step ({})",
                                        self.currency_code
                                    ))),
                            )
                            .child(Input::new(&self.step_minor)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_24()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(metrics::FONT_FOOTNOTE)
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from("Points per step")),
                            )
                            .child(Input::new(&self.points_per_step)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_24()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(metrics::FONT_FOOTNOTE)
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(format!(
                                        "Point value ({})",
                                        self.currency_code
                                    ))),
                            )
                            .child(Input::new(&self.point_value_minor)),
                    ),
            )
            .child(
                div()
                    .text_size(metrics::FONT_FOOTNOTE)
                    .text_color(crate::theme::text_dim())
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
                        div()
                            .text_size(metrics::FONT_FOOTNOTE)
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(
                                "Customer considered lost after (days without purchase)",
                            )),
                    )
                    .child(
                        div()
                            .w_24()
                            .child(Input::new(&self.lost_after_days)),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .pt_2()
                    .child(
                        Switch::new("addon-prompt")
                            .label("Offer the usual add-on after a product is added")
                            .checked(self.addon_prompt)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.addon_prompt = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Switch::new("ask-phone")
                            .label("Ask for a WhatsApp number at payment")
                            .checked(self.ask_phone)
                            .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                this.ask_phone = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_size(metrics::FONT_FOOTNOTE)
                            .text_color(crate::theme::text_dim())
                            .child("With Kaseer Cloud, these follow the owner's Grow settings."),
                    ),
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
                    .text_size(metrics::FONT_FOOTNOTE)
                    .text_color(crate::theme::text_dim())
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
                    .child(
                        TableCell::new().child(div().w_24().child(Input::new(&field.pct).small())),
                    )
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

        super::panel(cx)
            .gap(px(12.))
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        super::section_title(cx, "TAX RATES"),
                    )
                    .child(
                        div()
                            .text_size(metrics::FONT_FOOTNOTE)
                            .text_color(crate::theme::text_dim())
                            .child(SharedString::from(
                                "Percent: saved when a field loses focus",
                            )),
                    ),
            )
            .child(self.render_tax_table(cx, rows))
    }

    /// The cloud sync card: endpoint config, live outbox status, and the
    /// manual drain. Offline failures land in the status line: the
    /// cashier's sale path never blocks on the cloud.
    fn render_sync_panel(&self, cx: &Context<Self>) -> impl IntoElement {
        let (pending, last_sync, last_error) = self.sync_status();
        super::panel(cx)
            .gap(px(12.))
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        super::section_title(cx, "CLOUD SYNC"),
                    )
                    .child(
                        div()
                            .font_family(crate::theme::FONT_MONO)
                            .text_size(metrics::FONT_FOOTNOTE)
                            .text_color(crate::theme::text_dim())
                            .child(SharedString::from(format!(
                                "{} pending · last sync {}",
                                pending,
                                last_sync
                                    .as_deref()
                                    .map(short_time)
                                    .unwrap_or_else(|| "never".to_string()),
                            ))),
                    ),
            )
            .when_some(last_error, |panel, error| {
                panel.child(
                    div()
                        .text_size(metrics::FONT_FOOTNOTE)
                        .text_color(cx.theme().danger)
                        .child(SharedString::from(format!("Last error: {error}"))),
                )
            })
            .when(self.outbox.is_none(), |panel| {
                panel.child(
                    div()
                        .text_size(metrics::FONT_FOOTNOTE)
                        .text_color(cx.theme().danger)
                        .child("Sync database unavailable: sync disabled."),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_64()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(metrics::FONT_FOOTNOTE)
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Cloud URL"),
                            )
                            .child(Input::new(&self.cloud_url)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_64()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(metrics::FONT_FOOTNOTE)
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Store token"),
                            )
                            .child(Input::new(&self.cloud_token).mask_toggle()),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        div()
                            .text_size(metrics::FONT_FOOTNOTE)
                            .text_color(crate::theme::text_dim())
                            .child(SharedString::from(
                                self.sync_message.clone().unwrap_or_else(|| {
                                    "Fields save when they lose focus.".to_string()
                                }),
                            )),
                    )
                    .child(
                        Button::new("sync-now")
                            .label(if self.sync_in_flight {
                                "Syncing…"
                            } else {
                                "Sync now"
                            })
                            .secondary()
                            .small()
                            .disabled(self.sync_in_flight || self.outbox.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.sync_now(cx))),
                    ),
            )
            .child(self.render_menu_section(cx))
    }

    /// The menu pull section, inside the sync card: the active source's
    /// status line and the manual pull. Enrolled kiosks pull from the
    /// cloud with the device token (10s cadence) and never see the
    /// companion field; everyone else configures the LAN companion.
    fn render_menu_section(&self, cx: &Context<Self>) -> impl IntoElement {
        let (last_pull, last_error) = self.menu_status();
        let enrolled = self.kiosk_identity().is_some();
        v_flex()
            .gap_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .pt_3()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        super::section_title(
                            cx,
                            if enrolled {
                                "MENU (CLOUD)"
                            } else {
                                "MENU (COMPANION)"
                            },
                        ),
                    )
                    .child(
                        div()
                            .font_family(crate::theme::FONT_MONO)
                            .text_size(metrics::FONT_FOOTNOTE)
                            .text_color(crate::theme::text_dim())
                            .child(SharedString::from(format!(
                                "Menu source: {} · last pull {}",
                                if enrolled {
                                    "Cloud (enrolled), polls every 10s"
                                } else {
                                    "LAN companion, polls every 30s"
                                },
                                last_pull
                                    .as_deref()
                                    .map(short_time)
                                    .unwrap_or_else(|| "never".to_string()),
                            ))),
                    ),
            )
            .when_some(last_error, |section, error| {
                section.child(
                    div()
                        .text_size(metrics::FONT_FOOTNOTE)
                        .text_color(cx.theme().danger)
                        .child(SharedString::from(format!("Last error: {error}"))),
                )
            })
            .when(!enrolled, |section| {
                section.child(
                    v_flex()
                        .flex_1()
                        .min_w_64()
                        .gap_1()
                        .child(
                            div()
                                .text_size(metrics::FONT_FOOTNOTE)
                                .text_color(cx.theme().muted_foreground)
                                .child("Menu source (companion)"),
                        )
                        .child(Input::new(&self.menu_url)),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        div()
                            .text_size(metrics::FONT_FOOTNOTE)
                            .text_color(crate::theme::text_dim())
                            .child(SharedString::from(
                                self.menu_message.clone().unwrap_or_else(|| {
                                    "Polls every 30s; edits from this terminal push back."
                                        .to_string()
                                }),
                            )),
                    )
                    .child(
                        Button::new("menu-pull-now")
                            .label(if self.menu_in_flight {
                                "Pulling…"
                            } else {
                                "Pull menu now"
                            })
                            .secondary()
                            .small()
                            .disabled(self.menu_in_flight || self.outbox.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.pull_menu_now(cx))),
                    ),
            )
    }

    /// The cloud kiosk card: pairing-code enrollment before the terminal
    /// has an identity; kiosk name, store, last heartbeat, and pending
    /// sales once it has one. Ends with the About line and the update
    /// banner the background updater flips on.
    fn render_kiosk_panel(&self, cx: &Context<Self>) -> impl IntoElement {
        let pending = self.sync_status().0;
        let identity = self.kiosk_identity();
        let enrolled = identity.is_some();
        let last_heartbeat = self
            .outbox
            .as_ref()
            .and_then(|outbox| outbox.meta_get("kiosk_last_heartbeat"));
        let update_pending = self
            .outbox
            .as_ref()
            .and_then(|outbox| outbox.meta_get("update_pending"));

        super::panel(cx)
            .gap(px(12.))
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        super::section_title(cx, "KASEER CLOUD"),
                    )
                    .child(
                        div()
                            .font_family(crate::theme::FONT_MONO)
                            .text_size(metrics::FONT_FOOTNOTE)
                            .text_color(crate::theme::text_dim())
                            .child(SharedString::from(format!(
                                "{pending} sales pending sync · device token stays on this device",
                            ))),
                    ),
            )
            .when_some(update_pending, |panel, version| {
                panel.child(
                    div()
                        .text_size(metrics::FONT_EMPHASIS)
                        .font_weight(FontWeight::MEDIUM)
                        .child(SharedString::from(format!(
                            "Update {version} installed: restart to apply."
                        ))),
                )
            })
            .when(enrolled, |panel| {
                let identity = identity.clone().unwrap();
                let heartbeat = last_heartbeat
                    .clone()
                    .map(|iso| short_time(&iso))
                    .unwrap_or_else(|| "never".to_string());
                panel
                    .child(
                        div()
                            .text_size(metrics::FONT_EMPHASIS)
                            .font_weight(FontWeight::MEDIUM)
                            .child(SharedString::from(format!(
                                "Enrolled as {}",
                                identity.name,
                            ))),
                    )
                    .child(
                        div()
                            .font_family(crate::theme::FONT_MONO)
                            .text_size(metrics::FONT_FOOTNOTE)
                            .text_color(crate::theme::text_dim())
                            .child(SharedString::from(format!(
                                "Store {} · kiosk {} · last heartbeat {} · menu from cloud",
                                identity.store_id, identity.kiosk_id, heartbeat,
                            ))),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(metrics::FONT_FOOTNOTE)
                                    .text_color(crate::theme::text_dim())
                                    .child(SharedString::from(
                                        self.enroll_message
                                            .clone()
                                            .unwrap_or_else(|| {
                                                "Sales, menu, and knowledge flow through the cloud."
                                                    .to_string()
                                            }),
                                    )),
                            )
                            .child(
                                Button::new("kiosk-revoke")
                                    .label("Revoke")
                                    .secondary()
                                    .small()
                                    .disabled(self.enroll_in_flight)
                                    .on_click(cx.listener(|this, _, _, cx| this.revoke_now(cx))),
                            ),
                    )
            })
            .when(!enrolled, |panel| {
                panel
                    .child(
                        div()
                            .text_size(metrics::FONT_FOOTNOTE)
                            .text_color(crate::theme::text_dim())
                            .child(SharedString::from(
                                "Connect to Kaseer Cloud: create a kiosk on the cloud console, then enter its 6-digit pairing code here.",
                            )),
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
                                    .min_w_64()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(metrics::FONT_FOOTNOTE)
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Cloud URL"),
                                    )
                                    .child(Input::new(&self.enroll_url)),
                            )
                            .child(
                                v_flex()
                                    .min_w_32()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(metrics::FONT_FOOTNOTE)
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Pairing code"),
                                    )
                                    .child(Input::new(&self.pairing_code)),
                            ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(metrics::FONT_FOOTNOTE)
                                    .text_color(
                                        if self.enroll_message.as_deref().is_some_and(|m| m.starts_with("Enroll failed")) {
                                            cx.theme().danger
                                        } else {
                                            crate::theme::text_dim()
                                        },
                                    )
                                    .child(SharedString::from(
                                        self.enroll_message
                                            .clone()
                                            .unwrap_or_else(|| {
                                                format!(
                                                    "Default: KASEER_CLOUD_URL or {}.",
                                                    crate::kiosk::DEFAULT_CLOUD_URL
                                                )
                                            }),
                                    )),
                            )
                            .child(
                                Button::new("kiosk-enroll")
                                    .label(if self.enroll_in_flight {
                                        "Enrolling…"
                                    } else {
                                        "Enroll kiosk"
                                    })
                                    .secondary()
                                    .small()
                                    .disabled(self.enroll_in_flight || self.outbox.is_none())
                                    .on_click(cx.listener(|this, _, _, cx| this.enroll_now(cx))),
                            ),
                    )
            })
            .child(
                div()
                    .font_family(crate::theme::FONT_MONO)
                    .text_size(metrics::FONT_FOOTNOTE)
                    .text_color(crate::theme::text_dim())
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .pt_2()
                    .child(SharedString::from(format!(
                        "About: Kaseer {} · {} · update channel cloud (checked at boot and every 6h; never mid-shift)",
                        env!("CARGO_PKG_VERSION"),
                        crate::kiosk::arch(),
                    ))),
            )
    }

    fn render_tax_table(&self, cx: &Context<Self>, rows: Vec<TableRow>) -> impl IntoElement {
        let rates_ok = self.with_repo(|repo| repo.tax_rates().map(|rates| !rates.is_empty()));
        match rates_ok {
            Some(Ok(true)) => Table::new()
                .accessibility_label("Tax rates")
                .child(
                    TableHeader::new().child(
                        TableRow::new()
                            .child(TableHead::new()
                                    .text_size(metrics::FONT_FOOTNOTE)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(cx.theme().muted_foreground)
                                    .child("NAME"))
                            .child(TableHead::new()
                                    .text_size(metrics::FONT_FOOTNOTE)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(cx.theme().muted_foreground)
                                    .child("RATE %"))
                            .child(TableHead::new()
                                    .text_size(metrics::FONT_FOOTNOTE)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(cx.theme().muted_foreground)
                                    .child("TYPE")),
                    ),
                )
                .child(TableBody::new().children(rows))
                .into_any_element(),
            Some(Ok(false)) => div()
                .text_size(metrics::FONT_FOOTNOTE)
                .text_color(crate::theme::text_dim())
                .child(SharedString::from("No tax rates configured."))
                .into_any_element(),
            Some(Err(e)) => div()
                .text_size(metrics::FONT_FOOTNOTE)
                .text_color(cx.theme().danger)
                .child(SharedString::from(e.to_string()))
                .into_any_element(),
            None => div().into_any_element(),
        }
    }
}
