//! The register: scan-first selling. Search/barcode entry with exact-SKU
//! add on Enter, category chips, a product grid, and a live cart, closing
//! through the pay and receipt dialogs. Features are ported 1:1 from the
//! web register (`kaseer-web/src/web/screens/register.tsx` plus its pay and
//! receipt modals); the checkout transaction mirrors the web store exactly
//! and lands in `Repo::insert_sale`.
//!
//! State ownership: the ledger [`Repo`] lives inside this single view
//! entity (rusqlite's `Connection` is `!Sync`, so one owner). Queries run
//! in update paths and load snapshots into screen state; `render` only
//! derives presentation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::customers::SelectedCustomer;
use gpui_kit::assets::IconName;
use gpui_kit::base::{StyledExt as _, h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::searchable_list::SearchableListItem;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IndexPath, Sizable as _, WindowExt,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::prelude::StyledImage as _;
use gpui_kit::{
    Anchor, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, img, px,
};
use kaseer_core::grow::{AddonPair, addon_pairs, receipt_text, wa_phone, whatsapp_url};
use kaseer_core::cart::{self, Cart, CartTotals, Discount};
use kaseer_core::currency::{Currency, IDR, currency_by_code};
use kaseer_core::db::{open_repo, resolve_db_path};
use kaseer_core::escpos::escpos_bytes;
use kaseer_core::inventory::{StockTracking, is_low_stock};
use kaseer_core::loyalty::{
    LoyaltyConfig, max_redeemable_points, points_earned, redeem_value_minor,
};
use kaseer_core::menu;
use kaseer_core::money;
use kaseer_core::profit::{cart_profit, guarded_bps, max_order_discount_bps};
use kaseer_core::payments::{self, PaymentState, Tender, TenderMethod};
use kaseer_core::receipt::{AddonOffer, Sale, SaleLine, SaleTender, StoreInfo, render_receipt_text};
use kaseer_core::repos::{Customer, Product, Repo, Settings, WinbackOfferRow};
use kaseer_core::shift::Shift;
use kaseer_core::tax::TaxRate;
use kaseer_core::winback::{Recognition, recognition};

/// What the register asks of the shell. The total block's "Open shift"
/// control has to navigate, and navigation belongs to the shell.
pub enum RegisterEvent {
    OpenShift,
}

impl EventEmitter<RegisterEvent> for RegisterScreen {}

/// The register home: catalog on the left, current sale on the right, pay
/// and receipt dialogs above both.
pub struct RegisterScreen {
    repo: Repo,
    settings: Settings,
    currency: &'static Currency,
    tax_rates: Vec<TaxRate>,
    products: Vec<Product>,
    categories: Vec<String>,
    customers: Vec<Customer>,
    stock: HashMap<String, i64>,
    active_shift: Option<Shift>,
    cart: Cart,
    query: Entity<InputState>,
    category: Option<String>,
    percent: Entity<InputState>,
    customer_select: Entity<SelectState<Vec<CustomerItem>>>,
    pay_dialog: Entity<PayDialog>,
    receipt_dialog: Entity<ReceiptDialog>,
    /// Last menu-pull revision this snapshot reflects (see spawn_menu_watch).
    menu_rev_seen: u64,
    /// Revision parked by the watcher, consumed at the top of render.
    pending_menu_rev: Option<u64>,
    /// "With X, offer Y" from the last 30 days of sales (kaseer_core::grow).
    /// Computed off the UI thread at startup and after every settled sale;
    /// empty until then, which hides the add-on button.
    addon_pairs: HashMap<String, AddonPair>,
    /// The add-on on offer right now (partner product id), if any.
    addon_prompt: Option<String>,
    /// Add-ons offered during this cart; synced with the sale so the cloud
    /// learns the real yes-rate.
    addon_offers: Vec<AddonOffer>,
    /// Win-back code applied to this cart.
    offer: Option<WinbackOfferRow>,
    /// The whole-sale discount the cashier (or a code) asked for, basis
    /// points. The cart carries at most what the profit guard allows.
    asked_bps: Option<i64>,
    /// Product id -> HPP (cost), for the profit guard.
    costs: HashMap<String, i64>,
    /// The attached customer's live code, looked up once per customer.
    offer_hint: Option<(String, Option<WinbackOfferRow>)>,
    code_input: Entity<InputState>,
    /// The attached customer's visit summary, keyed by customer id so it
    /// is queried once per attach, not per frame.
    recognition: Option<(String, Recognition)>,
    /// Clear was tapped once; the next tap empties the cart.
    clear_armed: bool,
    _subscriptions: Vec<Subscription>,
}

impl RegisterScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let repo = open_repo(&resolve_db_path()).expect("failed to open the Kaseer ledger");
        let settings = repo.settings().expect("seeded settings row");
        let currency = currency_by_code(&settings.currency_code).unwrap_or(&IDR);
        let products = repo.products(false, None).unwrap_or_default();
        let categories = repo.categories().unwrap_or_default();
        let customers = repo.customers(None).unwrap_or_default();
        let tax_rates = repo.tax_rates().unwrap_or_default();
        let active_shift = repo.active_shift().unwrap_or_default();
        let stock = stock_map(&repo, &products);

        let query = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Scan barcode or search products, then Enter")
        });
        let percent = cx.new(|cx| InputState::new(window, cx).placeholder("Disc %"));
        let customer_select = cx.new(|cx| {
            SelectState::new(
                customer_items(&customers),
                Some(IndexPath::new(0)),
                window,
                cx,
            )
        });
        let receipts_dir = Path::new(repo.db_path())
            .parent()
            .map(|dir| dir.join("receipts"));
        let pay_dialog = cx.new(|cx| PayDialog::new(currency, window, cx));
        let receipt_dialog = cx.new(|_cx| ReceiptDialog::new(receipts_dir, currency));

        let code_input = cx.new(|cx| InputState::new(window, cx).placeholder("Code, e.g. K7QM3X"));
        let mut screen = Self {
            repo,
            settings,
            currency,
            tax_rates,
            products,
            categories,
            customers,
            stock,
            active_shift,
            cart: cart::empty_cart(currency.code),
            query,
            category: None,
            percent,
            customer_select,
            pay_dialog,
            receipt_dialog,
            menu_rev_seen: crate::menu_sync::applied_revision(),
            pending_menu_rev: None,
            addon_pairs: HashMap::new(),
            addon_prompt: None,
            addon_offers: Vec::new(),
            offer: None,
            asked_bps: None,
            costs: HashMap::new(),
            offer_hint: None,
            code_input,
            recognition: None,
            clear_armed: false,
            _subscriptions: Vec::new(),
        };

        screen.costs = cost_map(&screen.products);
        let query = screen.query.clone();
        screen._subscriptions.push(cx.subscribe_in(
            &query,
            window,
            |this, _, event, window, cx| {
                // Keyboard-wedge scanners type the SKU and press Enter.
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.scan_or_search(window, cx);
                }
            },
        ));

        let select = screen.customer_select.clone();
        screen
            ._subscriptions
            .push(
                cx.subscribe_in(&select, window, |this, _, event, _, cx| match event {
                    SelectEvent::Confirm(value) => {
                        let id = value.clone().filter(|id| !id.is_empty());
                        this.cart = cart::set_cart_customer(&this.cart, id);
                        this.drop_stale_offer();
                        cx.notify();
                    }
                }),
            );

        // Enter in the code field applies it, like the Apply button.
        let code = screen.code_input.clone();
        screen._subscriptions.push(cx.subscribe_in(&code, window, |this, input, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                let raw = input.read(cx).value().to_string();
                match this.apply_code(&raw, window, cx) {
                    Ok(()) => window.close_dialog(cx),
                    Err(message) => window.push_notification(Notification::error(message), cx),
                }
            }
        }));

        let pay = screen.pay_dialog.clone();
        screen
            ._subscriptions
            .push(cx.subscribe_in(&pay, window, |this, _, event, window, cx| {
                this.on_pay_event(event, window, cx)
            }));

        // The web register autofocuses the scan box.
        screen.query.update(cx, |input, cx| input.focus(window, cx));
        screen.spawn_menu_watch(cx);
        screen.refresh_bundles(cx);
        screen
    }

    // ── Data flow ─────────────────────────────────────────────────────────

    fn totals(&self) -> CartTotals {
        cart::compute_totals(&self.cart, &self.tax_rates, self.currency)
    }

    /// Live filter over the search box: category chip plus a
    /// name-or-SKU substring match (the web grid's `useMemo`). The result
    /// is grouped by category order (stable, so names stay in catalog order
    /// within a group): with every tile edged in its category hue, the "All"
    /// grid then reads as colour regions instead of confetti.
    fn filtered_products(&self, cx: &Context<Self>) -> Vec<Product> {
        let query = self.query.read(cx).value().trim().to_lowercase();
        let mut products: Vec<Product> = self.products
            .iter()
            .filter(|product| {
                let in_category = self
                    .category
                    .as_deref()
                    .is_none_or(|category| product.category == category);
                let matches = query.is_empty()
                    || product.name.to_lowercase().contains(&query)
                    || product.sku.to_lowercase().contains(&query);
                in_category && matches
            })
            .cloned()
            .collect();
        products.sort_by_key(|product| {
            self.categories
                .iter()
                .position(|category| *category == product.category)
                .unwrap_or(self.categories.len())
        });
        products
    }

    fn attached_customer(&self) -> Option<Customer> {
        let id = self.cart.customer_id.as_ref()?;
        self.customers.iter().find(|c| c.id == *id).cloned()
    }

    /// Shell hook, run when the register becomes the visible screen: this
    /// view is retained across navigation, while other screens (Shift,
    /// Inventory, Customers) mutate the ledger behind its back. Re-read the
    /// shift snapshot and catalog so the Pay gate and customer select are
    /// never stale.
    pub fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active_shift = self.repo.active_shift().unwrap_or_default();
        // Settings may have moved the lost-after window; re-derive the
        // recognition line against it.
        if let Ok(settings) = self.repo.settings() {
            self.settings.config = settings.config;
        }
        self.recognition = None;
        self.refresh_catalog(window, cx);
        // Coming back to the register means scanning next, as on boot.
        self.query.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Consume the Customers screen's cross-screen handoff: its "Sell to"
    /// action parks the pick in the [`SelectedCustomer`] global, and the
    /// next Register render attaches it to the cart (then clears the
    /// global, so the handoff fires once).
    fn consume_selected_customer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(customer) = SelectedCustomer::get(cx) else {
            return;
        };
        SelectedCustomer::set(cx, None);
        if self.customers.iter().any(|c| c.id == customer.id) {
            self.cart = cart::set_cart_customer(&self.cart, Some(customer.id));
            self.sync_customer_select(window, cx);
            cx.notify();
        }
    }

    /// Recompute co-purchase pairs from the full sale history on a
    /// background thread with its own ledger connection (the screen's
    /// connection is `!Sync`). The entity update only swaps the result in,
    /// so no ledger I/O runs inside it. A failed read keeps the old pairs.
    fn refresh_bundles(&self, cx: &mut Context<Self>) {
        // Kaseer Cloud's pairs, when it sent some: every terminal offers the same.
        if !self.settings.config.cloud_addons.is_empty() {
            let pairs: HashMap<String, AddonPair> = self
                .settings
                .config
                .cloud_addons
                .iter()
                .map(|a| {
                    (
                        a.anchor_id.clone(),
                        AddonPair { anchor_id: a.anchor_id.clone(), partner_id: a.partner_id.clone(), attach_pct: a.attach_pct },
                    )
                })
                .collect();
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.addon_pairs = pairs;
                    cx.notify();
                });
            })
            .detach();
            return;
        }
        let path = self.repo.db_path().to_string();
        cx.spawn(async move |this, cx| {
            let pairs = cx
                .background_executor()
                .spawn(async move {
                    let repo = open_repo(&path).ok()?;
                    let since = civil_timestamp((unix_nanos() / 1_000_000_000) as i64 - 30 * 86_400);
                    let sales = repo.sales_between(&since, "9999-12-31T23:59:59.999Z").ok()?;
                    let products = repo.products(true, None).ok()?;
                    Some(addon_pairs(&sales, &products))
                })
                .await;
            if let Some(pairs) = pairs {
                let _ = this.update(cx, |this, cx| {
                    this.addon_pairs = pairs;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Keep the recognition cache on the cart's customer. Runs at the top
    /// of render like the other snapshot consumers: one indexed query when
    /// the customer changes, nothing on other frames. A failed query
    /// leaves the header quiet rather than guessing.
    fn sync_recognition(&mut self) {
        let Some(id) = self.cart.customer_id.clone() else {
            self.recognition = None;
            return;
        };
        if self.recognition.as_ref().is_some_and(|(cached, _)| *cached == id) {
            return;
        }
        let lost_after_days = self.settings.config.lost_after_days;
        self.recognition = self.repo.customer_visit_summary(&id).ok().map(|(visits, last)| {
            (id, recognition(visits, last.as_deref(), lost_after_days, &now_iso()))
        });
    }

    /// The add-on button's product, while it is still worth offering.
    fn addon_product(&self) -> Option<Product> {
        let partner = self.addon_prompt.as_ref()?;
        if self.cart.lines.iter().any(|l| &l.product_id == partner) {
            return None;
        }
        self.products.iter().find(|p| &p.id == partner && !p.archived).cloned()
    }

    /// After a product lands in the cart: offer its usual add-on, once per cart.
    fn maybe_offer_addon(&mut self, product_id: &str) {
        if !self.settings.config.addon_prompt {
            return;
        }
        let Some(pair) = self.addon_pairs.get(product_id) else {
            return;
        };
        let in_cart = self.cart.lines.iter().any(|l| l.product_id == pair.partner_id);
        let offered = self.addon_offers.iter().any(|o| o.partner_id == pair.partner_id);
        if in_cart || offered {
            return;
        }
        // A button replaced before anyone answered it was never really
        // offered: it must not count as a "no" in the yes-rate.
        if let Some(previous) = self.addon_prompt.take()
            && let Some(ix) = self.addon_offers.iter().rposition(|o| o.partner_id == previous && !o.accepted)
        {
            self.addon_offers.remove(ix);
        }
        self.addon_offers.push(AddonOffer {
            anchor_id: pair.anchor_id.clone(),
            partner_id: pair.partner_id.clone(),
            accepted: false,
        });
        self.addon_prompt = Some(pair.partner_id.clone());
    }

    /// The largest whole-sale discount that keeps the owner's minimum
    /// profit over HPP; None when the guard is off or no item has a cost.
    fn max_discount_bps(&self) -> Option<i64> {
        let config = &self.settings.config;
        if !config.profit_guard {
            return None;
        }
        max_order_discount_bps(&self.cart, &self.costs, &self.tax_rates, self.currency, config.min_margin_pct)
    }

    /// The cart carries the asked discount, or the guard's limit when that
    /// is lower. Runs at the top of every render, so adding a thin-margin
    /// item lowers the discount and adding a fat one gives it back.
    fn apply_guard(&mut self) {
        let applied = guarded_bps(self.asked_bps, self.max_discount_bps()).map(|bps| Discount::Percent { bps });
        if self.cart.order_discount != applied {
            self.cart = cart::set_order_discount(&self.cart, applied);
        }
    }

    /// What the guard did to this cart, in words for the cashier.
    fn guard_note(&self) -> Option<SharedString> {
        let config = &self.settings.config;
        if !config.profit_guard || self.cart.lines.is_empty() {
            return None;
        }
        let floor = config.min_margin_pct;
        let applied = match self.cart.order_discount {
            Some(Discount::Percent { bps }) => bps,
            _ => 0,
        };
        if let Some(asked) = self.asked_bps.filter(|asked| *asked > applied) {
            return Some(SharedString::from(if applied == 0 {
                format!("No discount on this sale: {}% would cut under your {floor}% minimum profit", percent_label(asked))
            } else {
                format!("Discount lowered to {}% to keep at least {floor}% profit", percent_label(applied))
            }));
        }
        let profit = cart_profit(&self.cart, &self.costs, &self.tax_rates, self.currency)?;
        (!profit.meets(floor)).then(|| {
            SharedString::from(format!(
                "Under your {floor}% minimum profit: {} over HPP",
                fmt_money(profit.profit_minor(), self.currency)
            ))
        })
    }

    /// A fresh cart: no add-on on offer, no code.
    fn reset_grow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.addon_prompt = None;
        self.addon_offers.clear();
        self.offer = None;
        self.asked_bps = None;
        self.code_input.update(cx, |input, cx| input.set_value("", window, cx));
    }

    /// Redeem a win-back code: attach its customer and take its discount.
    fn apply_code(&mut self, raw: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let code = kaseer_core::repos::normalize_code(raw);
        if code.is_empty() {
            return Err("Type the code from the customer's WhatsApp".to_string());
        }
        let offer = self
            .repo
            .live_winback_offer(&code, &now_iso())
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{code} is not a valid code (used, expired, or not synced yet)"))?;
        // The cloud's customer may not be on this terminal yet.
        if self.repo.customer_by_id(&offer.customer_id).map_err(|e| e.to_string())?.is_none() {
            let customer = Customer {
                id: offer.customer_id.clone(),
                name: if offer.customer_name.is_empty() { offer.customer_phone.clone() } else { offer.customer_name.clone() },
                phone: offer.customer_phone.clone(),
                points: 0,
                created_at: now_iso(),
            };
            self.repo.upsert_customer(&customer).map_err(|e| e.to_string())?;
            self.customers = self.repo.customers(None).unwrap_or_default();
        }
        if self.max_discount_bps() == Some(0) {
            return Err(format!(
                "{code} can't be used on this sale: any discount would take it under your {}% minimum profit",
                self.settings.config.min_margin_pct
            ));
        }
        self.cart = cart::set_cart_customer(&self.cart, Some(offer.customer_id.clone()));
        self.sync_customer_select(window, cx);
        self.set_order_percent(offer.pct * 100, window, cx);
        self.offer = Some(offer);
        self.offer_hint = None;
        cx.notify();
        Ok(())
    }

    /// The attached customer's live win-back code, when none is applied yet.
    fn pending_offer(&self) -> Option<WinbackOfferRow> {
        if self.offer.is_some() {
            return None;
        }
        let (id, offer) = self.offer_hint.as_ref()?;
        (self.cart.customer_id.as_ref() == Some(id)).then(|| offer.clone()).flatten()
    }

    /// Look the attached customer's code up once per customer, not per frame.
    fn sync_offer_hint(&mut self) {
        let Some(id) = self.cart.customer_id.clone() else {
            self.offer_hint = None;
            return;
        };
        if self.offer_hint.as_ref().is_some_and(|(cached, _)| *cached == id) {
            return;
        }
        let offer = self.repo.live_winback_offer_for(&id, &now_iso()).ok().flatten();
        self.offer_hint = Some((id, offer));
    }

    /// The applied code still matches the sale: its customer, its discount.
    fn offer_applies(&self) -> bool {
        let Some(offer) = &self.offer else {
            return false;
        };
        self.cart.customer_id.as_deref() == Some(offer.customer_id.as_str())
            && self.asked_bps == Some(offer.pct * 100)
    }

    /// The cashier changed the customer or the discount: the code is off.
    fn drop_stale_offer(&mut self) {
        if self.offer.is_some() && !self.offer_applies() {
            self.offer = None;
        }
    }

    /// A WhatsApp number typed at payment: find that customer, or add one.
    fn attach_phone(&mut self, phone: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        if wa_phone(phone, self.currency.code).is_none() {
            return Err(format!("{phone} does not look like a WhatsApp number"));
        }
        let customer = match self.repo.customer_by_phone(phone).map_err(|e| e.to_string())? {
            Some(found) => found,
            None => {
                // Nameless: the number is not a name (messages would read
                // "Halo 0812…"); the Customers screen can add one later.
                let c = Customer {
                    id: format!("cus-{}", unix_nanos()),
                    name: String::new(),
                    phone: phone.trim().to_string(),
                    points: 0,
                    created_at: now_iso(),
                };
                self.repo.upsert_customer(&c).map_err(|e| e.to_string())?
            }
        };
        self.customers = self.repo.customers(None).unwrap_or_default();
        self.cart = cart::set_cart_customer(&self.cart, Some(customer.id));
        self.drop_stale_offer();
        self.sync_customer_select(window, cx);
        Ok(())
    }

    /// Units per product in the cart: the tiles' quantity badges.
    fn cart_qty_by_product(&self) -> HashMap<String, f64> {
        let mut qty: HashMap<String, f64> = HashMap::new();
        for line in &self.cart.lines {
            *qty.entry(line.product_id.clone()).or_insert(0.0) += line.qty;
        }
        qty
    }

    fn load_stock(&self) -> HashMap<String, i64> {
        stock_map(&self.repo, &self.products)
    }

    /// Menu pull watch: the pull engine writes the ledger on a background
    /// thread and records the applied revision in a process global. The
    /// tick itself is deliberately trivial (a field write + notify, the
    /// same shape as Settings' refresh loop): an async entity update must
    /// never run ledger I/O or heavy work, or its flushed frame can
    /// re-enter Root mid-update. The snapshot reload happens in
    /// [`Self::consume_pending_menu_revision`] at the top of this view's
    /// own render pass, single-threaded. The loop ends when the screen
    /// entity drops.
    fn spawn_menu_watch(&self, cx: &mut Context<Self>) {
        let mut seen = crate::menu_sync::applied_revision();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let rev = crate::menu_sync::applied_revision();
                if rev == seen {
                    continue;
                }
                seen = rev;
                if this
                    .update(cx, |this, cx| {
                        this.pending_menu_rev = Some(rev);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Called at the top of `render`: reload the catalog snapshot when the
    /// watcher parked a menu revision. Runs inside this view's own render
    /// pass: one owner, one thread, no re-entrant entity updates. A failed
    /// read keeps the previous snapshot.
    fn consume_pending_menu_revision(&mut self, cx: &mut Context<Self>) {
        let Some(rev) = self.pending_menu_rev.take() else {
            return;
        };
        self.menu_rev_seen = rev;
        // The pull may carry the owner's switches, add-on pairs and new codes.
        if let Ok(settings) = self.repo.settings() {
            self.settings = settings;
        }
        self.offer_hint = None;
        self.refresh_bundles(cx);
        if let Ok(products) = self.repo.products(false, None) {
            self.costs = cost_map(&products);
            self.products = products;
        }
        if let Ok(categories) = self.repo.categories() {
            self.categories = categories;
        }
        self.stock = self.load_stock();
    }

    /// Re-read catalog, customers, and stock after a settled sale. A failed
    /// read keeps the previous snapshot instead of blanking the screen.
    fn refresh_catalog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(products) = self.repo.products(false, None) {
            self.costs = cost_map(&products);
            self.products = products;
        }
        if let Ok(customers) = self.repo.customers(None) {
            self.customers = customers;
        }
        if let Ok(categories) = self.repo.categories() {
            self.categories = categories;
        }
        self.stock = self.load_stock();
        self.sync_customer_select(window, cx);
        cx.notify();
    }

    /// Point the customer select back at the cart's customer (Walk-in after
    /// a settled sale). `set_selected_index` is owner-driven and emits no
    /// Confirm, so this cannot feed back through the subscription.
    fn sync_customer_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = customer_items(&self.customers);
        let wanted = self.cart.customer_id.clone().unwrap_or_default();
        let index = items.iter().position(|item| item.id == wanted).unwrap_or(0);
        self.customer_select.update(cx, |select, cx| {
            select.set_items(items, window, cx);
            select.set_selected_index(Some(IndexPath::new(index)), window, cx);
        });
    }

    // ── Cart mutations ────────────────────────────────────────────────────

    /// Enter in the scan box adds an exact SKU match to the cart, then
    /// clears the field (the web register's exact-SKU contract).
    fn scan_or_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let term = self.query.read(cx).value().trim().to_lowercase();
        if term.is_empty() {
            return;
        }
        let exact = self
            .products
            .iter()
            .find(|p| p.sku.to_lowercase() == term)
            .cloned();
        let Some(product) = exact else {
            return;
        };
        self.add_product(&product, window, cx);
    }

    fn add_product(&mut self, product: &Product, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_armed = false;
        match cart::add_to_cart(&self.cart, &product.addable(), 1.0) {
            Ok(next) => {
                self.cart = next;
                // Taken while it was on offer (the button, or its tile).
                if self.addon_prompt.as_deref() == Some(product.id.as_str()) {
                    self.addon_prompt = None;
                    if let Some(offer) = self.addon_offers.iter_mut().rev().find(|o| o.partner_id == product.id) {
                        offer.accepted = true;
                    }
                }
                self.maybe_offer_addon(&product.id);
                cx.notify();
            }
            Err(error) => window.push_notification(Notification::error(error.to_string()), cx),
        }
        self.query
            .update(cx, |input, cx| input.set_value("", window, cx));
    }

    fn set_line_qty(&mut self, line_id: &str, qty: f64, cx: &mut Context<Self>) {
        self.clear_armed = false;
        // At the 9_999 cap an extra + is ignored rather than crashing the
        // register (the web version would panic inside its state updater).
        if let Ok(next) = cart::set_line_qty(&self.cart, line_id, qty) {
            self.cart = next;
            cx.notify();
        }
    }

    fn apply_order_percent(&mut self, cx: &mut Context<Self>) {
        let raw = self.percent.read(cx).value().trim().to_string();
        let bps = raw
            .parse::<f64>()
            .map(|value| (value * 100.0).round() as i64)
            .unwrap_or(0)
            .clamp(0, 10_000);
        self.asked_bps = (bps > 0).then_some(bps);
        self.apply_guard();
        self.drop_stale_offer();
        cx.notify();
    }

    fn set_order_percent(&mut self, bps: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.asked_bps = Some(bps);
        self.apply_guard();
        let label = percent_label(bps);
        self.percent
            .update(cx, |input, cx| input.set_value(label, window, cx));
        self.drop_stale_offer();
        cx.notify();
    }

    /// First tap arms, second tap empties the cart (and its discount).
    fn tap_clear(&mut self, cx: &mut Context<Self>) {
        if self.clear_armed {
            self.cart = cart::clear_cart(&self.cart);
            self.asked_bps = None;
            self.clear_armed = false;
            self.addon_prompt = None;
            self.addon_offers.clear();
            self.offer = None;
        } else {
            self.clear_armed = true;
        }
        cx.notify();
    }

    fn clear_order_percent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.asked_bps = None;
        self.cart = cart::set_order_discount(&self.cart, None);
        self.percent
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.drop_stale_offer();
        cx.notify();
    }

    // ── Pay flow ──────────────────────────────────────────────────────────

    pub fn open_pay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Same lock as the Pay button: F2 must not open an empty or
        // shift-less payment.
        if self.cart.lines.is_empty() || self.active_shift.is_none() {
            return;
        }
        let total_minor = self.totals().total_minor;
        let customer = self.attached_customer().map(|c| CustomerSnapshot {
            id: c.id,
            name: c.name,
            points: c.points,
        });
        let loyalty = self.settings.loyalty.clone();
        let currency = self.currency;
        let ask_phone = self.settings.config.ask_phone;
        self.pay_dialog.update(cx, |dialog, cx| {
            dialog.reset(total_minor, customer, loyalty, currency, window, cx);
            dialog.set_ask_phone(ask_phone, cx);
        });
        open_pay_dialog(self.pay_dialog.clone(), window, cx);
    }

    fn on_pay_event(&mut self, event: &PayEvent, window: &mut Window, cx: &mut Context<Self>) {
        let PayEvent::Paid {
            tenders,
            redeem_points,
            phone,
        } = event;
        // A mistyped number never holds up the sale: settle as a walk-in.
        if let Some(phone) = phone
            && let Err(message) = self.attach_phone(phone, window, cx)
        {
            window.push_notification(Notification::warning(format!("{message}; sold as walk-in")), cx);
        }
        match self.checkout(tenders, *redeem_points, cx) {
            Ok(sale) => {
                window.close_dialog(cx);
                self.reset_grow(window, cx);
                self.refresh_catalog(window, cx);
                // The new sale is history now: pairs and visit counts move.
                self.refresh_bundles(cx);
                self.recognition = None;
                self.open_receipt(sale, window, cx);
            }
            Err(message) => {
                // Let the cashier fix it and try again.
                self.pay_dialog.update(cx, |pay, _| pay.unsettle());
                window.push_notification(Notification::error(message), cx);
            }
        }
    }

    /// Settle the current sale, mirroring the web store's `checkout`
    /// exactly: loyalty redemption rides in as its own tender, payment is
    /// validated, then the sale, tracked stock, and loyalty points land in
    /// one `Repo::insert_sale` transaction.
    fn checkout(
        &mut self,
        tenders: &[Tender],
        redeem_points: i64,
        cx: &mut Context<Self>,
    ) -> Result<Sale, String> {
        let Some(shift) = self.active_shift.clone() else {
            return Err("No open shift. Open one from the Shift screen first.".to_string());
        };
        let totals = self.totals();
        let customer = self.attached_customer();
        let loyalty = &self.settings.loyalty;
        let redeemable = redeem_points.max(0).min(max_redeemable_points(
            customer.as_ref().map(|c| c.points).unwrap_or(0),
            totals.net_minor,
            loyalty,
        ));
        let redeem_value = redeem_value_minor(redeemable, loyalty);
        if redeem_points > 0 && customer.is_none() {
            return Err("Attach a customer before redeeming points".to_string());
        }

        let mut effective: Vec<Tender> = Vec::with_capacity(tenders.len() + 1);
        if redeem_value > 0 {
            effective.push(payments::new_tender(
                TenderMethod::Loyalty,
                redeem_value,
                None,
            ));
        }
        effective.extend(tenders.iter().cloned());

        let payment = payments::evaluate_payments(totals.total_minor, &effective);
        if let Some(invalid) = payment.invalid {
            return Err(invalid);
        }
        if !payment.complete {
            return Err("Tenders do not cover the total yet".to_string());
        }

        let earned = points_earned(totals.net_minor, loyalty);
        let (number, _seq) = self.repo.next_sale_number().map_err(|e| e.to_string())?;

        let lines: Vec<SaleLine> = totals
            .lines
            .iter()
            .zip(&self.cart.lines)
            .map(|(line_totals, line)| SaleLine {
                product_id: line.product_id.clone(),
                name: line.name.clone(),
                sku: line.sku.clone(),
                qty: line.qty,
                unit_minor: line.unit_minor,
                cost_minor: None,
                discount_minor: line_totals.line_discount_minor + line_totals.order_discount_minor,
                net_minor: line_totals.net_minor,
                tax_minor: line_totals.tax_minor,
                tax_rate_id: line.tax_rate_id.clone(),
            })
            .collect();
        let sale_tenders: Vec<SaleTender> = effective
            .iter()
            .map(|tender| SaleTender {
                method: tender.method,
                applied_minor: tender.applied_minor.min(totals.total_minor),
                given_minor: tender.given_minor,
            })
            .collect();

        let sale = Sale {
            id: format!("sale-{}", unix_nanos()),
            number,
            at: now_iso(),
            currency: self.currency.code.to_string(),
            shift_id: Some(shift.id),
            customer_id: customer.as_ref().map(|c| c.id.clone()),
            customer_name_snapshot: customer.as_ref().map(|c| c.name.clone()),
            lines,
            tenders: sale_tenders,
            gross_minor: totals.gross_minor,
            discount_minor: totals.discount_minor,
            net_minor: totals.net_minor,
            tax_minor: totals.tax_minor,
            total_minor: totals.total_minor,
            change_minor: payment.change_minor,
            points_earned: earned,
            points_redeemed: redeemable,
            tax_by_rate_id: totals.tax_by_rate_id,
            // Only while the code's customer and discount are still on the sale.
            // A code the guard took to 0% gave nothing, so it stays unused.
            offer_code: (self.offer_applies() && self.cart.order_discount.is_some())
                .then(|| self.offer.as_ref().map(|o| o.code.clone()))
                .flatten(),
            // Taken while offered, and still in the cart at payment.
            addon_offers: self
                .addon_offers
                .iter()
                .map(|o| AddonOffer {
                    accepted: o.accepted && self.cart.lines.iter().any(|l| l.product_id == o.partner_id),
                    ..o.clone()
                })
                .collect(),
        };

        // Stock movements for tracked products (tracked items sell whole).
        let mut tracked: HashMap<String, f64> = HashMap::new();
        for line in &self.cart.lines {
            if let Some(product) = self.products.iter().find(|p| p.id == line.product_id)
                && product.track_stock
            {
                *tracked.entry(line.product_id.clone()).or_insert(0.0) += line.qty.round();
            }
        }

        // A failed write must not lose the cart, unlike the web flow where
        // persistence is fire-and-forget behind the server.
        self.repo
            .insert_sale(&sale, &tracked, earned - redeemable)
            .map_err(|error| error.to_string())?;

        self.cart = cart::clear_cart(&self.cart);
        self.asked_bps = None;
        cx.notify();
        Ok(sale)
    }

    fn open_receipt(&mut self, sale: Sale, window: &mut Window, cx: &mut Context<Self>) {
        let store = StoreInfo {
            name: self.settings.store_name.clone(),
            address: non_empty(self.settings.address.clone()),
            footer: non_empty(self.settings.receipt_footer.clone()),
        };
        let number = sale.number.clone();
        let whatsapp = sale
            .customer_id
            .as_ref()
            .and_then(|id| self.repo.customer_by_id(id).ok().flatten())
            .and_then(|c| wa_phone(&c.phone, self.currency.code));
        self.receipt_dialog.update(cx, |dialog, _| {
            dialog.show(sale, store);
            dialog.set_whatsapp(whatsapp);
        });
        let receipt = self.receipt_dialog.clone();
        let surface = cx.theme().popover;
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title(SharedString::from(format!("Receipt {number}")))
                .w(px(560.))
                .bg(surface)
                .child(receipt.clone())
        });
    }

    // ── Rendering ─────────────────────────────────────────────────────────
    //
    // Revenue register: the catalog reads by category colour (a 4px hue
    // edge per tile, a matching dot per chip), prices sit in plain text so
    // the accent only marks state (in-cart tiles, the active chip, focus),
    // and the cart column is anchored by the sale's money: suggestions from
    // real co-purchase history, then the one gradient in the app, the total.

    fn render_catalog(&self, cx: &Context<Self>, products: &[Product], columns: u16) -> impl IntoElement {
        use crate::theme::touch;
        let theme = cx.theme();
        let shift_pill = match &self.active_shift {
            Some(shift) => h_flex()
                .flex_none()
                .h(px(60.))
                .px(px(18.))
                .gap(px(10.))
                .items_center()
                .rounded(touch::RADIUS)
                .bg(theme.secondary)
                .text_size(touch::FONT_LABEL)
                .text_color(theme.muted_foreground)
                .child(div().size(px(10.)).rounded(px(999.)).bg(theme.success))
                .child(SharedString::from(format!(
                    "Shift open since {}",
                    clock_of(&shift.opened_at)
                )))
                .into_any_element(),
            None => h_flex()
                .id("shift-pill-open")
                .cursor_pointer()
                .flex_none()
                .h(px(60.))
                .px(px(18.))
                .gap(px(10.))
                .items_center()
                .rounded(touch::RADIUS)
                .border_1()
                .border_color(theme.warning.opacity(0.5))
                .text_size(touch::FONT_LABEL)
                .text_color(theme.warning)
                .child(div().size(px(10.)).rounded(px(999.)).bg(theme.warning))
                .child("No shift open")
                .on_click(cx.listener(|_, _, _, cx| cx.emit(RegisterEvent::OpenShift)))
                .into_any_element(),
        };
        v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(14.))
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(12.))
                    // The input's editor renders size_full: placed bare in a
                    // flex column it would take the whole column height and
                    // starve the grid. The fixed-height frame pins it.
                    .child(
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
                                Icon::new(IconName::ScanBarcode)
                                    .with_size(px(22.))
                                    .text_color(theme.muted_foreground),
                            )
                            .child(
                                Input::new(&self.query)
                                    .id("search")
                                    .appearance(false)
                                    .text_size(px(17.))
                                    .flex_1(),
                            ),
                    )
                    .child(shift_pill),
            )
            .child(self.render_category_chips(cx))
            .child(self.render_product_grid(cx, products, columns))
    }

    /// One category button. Category buttons lead with their hue dot so the
    /// row doubles as the colour key for the grid; the active one is the
    /// accent-filled button.
    fn render_category_chip(
        &self,
        cx: &Context<Self>,
        id: SharedString,
        label: SharedString,
        hue: Option<Hsla>,
        active: bool,
        category: Option<String>,
    ) -> impl IntoElement {
        use crate::theme::touch;
        let theme = cx.theme();
        let hover = theme.secondary_hover;
        h_flex()
            .id(id)
            .cursor_pointer()
            .flex_none()
            .h(touch::TAP)
            .px(px(22.))
            .gap(px(10.))
            .items_center()
            .rounded(touch::RADIUS)
            .text_size(touch::FONT_BODY)
            .font_weight(FontWeight::MEDIUM)
            .map(|chip| {
                if active {
                    chip.bg(theme.primary).text_color(theme.primary_foreground)
                } else {
                    chip.bg(theme.secondary)
                        .text_color(theme.muted_foreground)
                        .hover(move |style| style.bg(hover))
                }
            })
            .when_some(hue, |chip, hue| {
                chip.child(div().size(px(10.)).flex_none().rounded(px(999.)).bg(hue))
            })
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.category = category.clone();
                cx.notify();
            }))
    }

    fn render_category_chips(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut row = h_flex()
            .id("category-row")
            .gap(px(10.))
            .flex_none()
            .overflow_x_scroll();
        row = row.child(self.render_category_chip(
            cx,
            "cat-all".into(),
            "All".into(),
            None,
            self.category.is_none(),
            None,
        ));
        for name in &self.categories {
            row = row.child(self.render_category_chip(
                cx,
                SharedString::from(format!("cat-{name}")),
                SharedString::from(name.clone()),
                Some(crate::theme::category_hue(&self.categories, name)),
                self.category.as_deref() == Some(name.as_str()),
                Some(name.clone()),
            ));
        }
        row
    }

    fn render_product_grid(&self, cx: &Context<Self>, products: &[Product], columns: u16) -> impl IntoElement {
        if products.is_empty() {
            return div()
                .flex_grow(1.)
                .min_h_0()
                .pt_8()
                .text_center()
                .text_size(crate::theme::touch::FONT_BODY)
                .text_color(cx.theme().muted_foreground)
                .child("No products match.")
                .into_any_element();
        }
        // Rows pack to the top at their content height, so a single search
        // hit stays tile-sized instead of ballooning to the grid height.
        let mut grid = div()
            .grid()
            .grid_cols(columns)
            .content_start()
            .gap(px(12.))
            .flex_grow(1.)
            .min_h_0()
            .overflow_y_scrollbar();
        // Once any tile has a photo, photo-less tiles get a placeholder
        // band so names and prices sit on the same line across a row.
        let banded = products.iter().any(|product| self.photo_path(product).is_some());
        let in_cart = self.cart_qty_by_product();
        for product in products {
            let qty = in_cart.get(&product.id).copied();
            grid = grid.child(self.render_product_card(product, banded, qty, cx));
        }
        grid.into_any_element()
    }

    /// The product's downloaded photo, if the pull has landed it on disk.
    fn photo_path(&self, product: &Product) -> Option<PathBuf> {
        menu::local_image_path(self.repo.db_path(), &product.image_path)
            .filter(|path| path.exists())
    }

    /// Stock note for a tile, only when it needs the cashier's attention:
    /// tracked and at or under its low-stock threshold (the inventory
    /// screen's `is_low_stock`). Healthy stock stays off the tile.
    fn low_stock_note(&self, product: &Product) -> Option<String> {
        let on_hand = *self.stock.get(&product.id)?;
        let tracking = StockTracking {
            track_stock: product.track_stock,
            low_stock_at: product.low_stock_at,
        };
        is_low_stock(&tracking, on_hand).then(|| {
            if on_hand <= 0 {
                "Out of stock".to_string()
            } else {
                format!("Only {on_hand} left")
            }
        })
    }

    /// A product tile: the whole tile is the tap target. Name (two lines),
    /// price at the bottom, the category hue as a 6px band on the left, an
    /// in-cart count badge and the knowledge button in the top-right corner.
    fn render_product_card(
        &self,
        product: &Product,
        banded: bool,
        in_cart: Option<f64>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        use crate::theme::touch;
        let theme = cx.theme();
        let known = has_knowledge(product);
        let hue = crate::theme::category_hue(&self.categories, &product.category);
        let low_note = self.low_stock_note(product);
        let photo = self.photo_path(product);
        let body = v_flex()
            .w_full()
            .flex_grow(1.)
            .gap(px(8.))
            .pt(px(14.))
            .pb(px(14.))
            .pl(px(16.))
            .pr(px(14.))
            // Corner overlays (badge, knowledge button) never sit on the
            // name: reserve their width on the first lines.
            // The count badge sits in the top-right corner: keep the name
            // clear of it. The knowledge button sits bottom-right.
            .when(in_cart.is_some(), |body| body.pr(px(50.)))
            .child(
                div()
                    .text_size(touch::FONT_TILE)
                    .line_height(px(23.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_clamp(2)
                    .child(SharedString::from(product.name.clone())),
            )
            .child(
                h_flex()
                    .mt_auto()
                    .gap_2()
                    .items_end()
                    .justify_between()
                    .child(
                        super::money_text(fmt_money(product.price_minor, self.currency))
                            .flex_none()
                            .text_size(px(19.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .text_color(theme.danger)
                            .when_some(low_note, |note, text| note.child(SharedString::from(text))),
                    ),
            );
        // Category band: the shell is filled with the hue and the surface
        // sits 6px in from its left, so the hue reads as a band inside the
        // rounded corners (GPUI clips overflow to a rectangle, so a child
        // pinned to the left would poke out square at the corners).
        let inner_radius = touch::RADIUS_LG - px(1.);
        let surface = v_flex()
            .flex_1()
            .min_w_0()
            .ml(px(6.))
            .bg(theme.secondary)
            .rounded_tr(inner_radius)
            .rounded_br(inner_radius)
            .map(|surface| match photo {
                Some(path) => surface.child(
                    img(path)
                        .id(SharedString::from(format!("product-img-{}", product.id)))
                        .w_full()
                        .h(px(104.))
                        .flex_none()
                        .rounded_tr(inner_radius)
                        .object_fit(ObjectFit::Cover),
                ),
                None if banded => surface.child(
                    div()
                        .w_full()
                        .h(px(104.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_tr(inner_radius)
                        .bg(theme.muted)
                        .text_size(px(26.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(crate::theme::text_dim())
                        .child(SharedString::from(initials(&product.name))),
                ),
                None => surface,
            })
            .child(body);
        let card = h_flex()
            .id(SharedString::from(format!("product-{}", product.id)))
            .size_full()
            .items_stretch()
            .cursor_pointer()
            .min_h(touch::TILE_H)
            .overflow_hidden()
            .rounded(touch::RADIUS_LG)
            .bg(hue)
            .border_2()
            .map(|card| {
                if in_cart.is_some() {
                    card.border_color(theme.accent)
                } else {
                    card.border_color(theme.secondary)
                        .hover(|style| style.border_color(crate::theme::accent_dim()))
                }
            })
            .active(|style| style.opacity(0.8))
            .on_click(cx.listener({
                let product_id = product.id.clone();
                move |this, _, window, cx| {
                    if let Some(product) =
                        this.products.iter().find(|p| p.id == product_id).cloned()
                    {
                        this.add_product(&product, window, cx);
                    }
                }
            }))
            .child(surface);

        let badge = in_cart.map(|qty| {
            div()
                .absolute()
                .top(px(10.))
                .right(px(10.))
                .h(px(34.))
                .min_w(px(34.))
                .px(px(8.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(999.))
                .bg(theme.accent)
                .text_color(theme.accent_foreground)
                .font_family(crate::theme::FONT_MONEY)
                .font_features(crate::theme::tabular())
                .text_size(px(17.))
                .font_weight(FontWeight::BOLD)
                .child(SharedString::from(qty_label(qty)))
        });
        let knowledge = known.then(|| {
            div()
                .absolute()
                .bottom(px(10.))
                .right(px(10.))
                .child(self.render_knowledge_popover(product, cx))
        });
        div()
            .relative()
            .h_full()
            .child(card)
            .children(badge)
            .children(knowledge)
    }

    /// The knowledge affordance: an Info corner button floating over the
    /// card's top-right (the web `.kinfo` position), opening a click-away
    /// knowledge popover: the register.tsx `openPeek` flow on gpui-kit's
    /// Popover.
    fn render_knowledge_popover(&self, product: &Product, cx: &Context<Self>) -> impl IntoElement {
        let product = product.clone();
        let (muted, foreground) = (cx.theme().muted_foreground, cx.theme().foreground);
        let button_id = SharedString::from(format!("kinfo-btn-{}", product.id));
        Popover::new(SharedString::from(format!("kinfo-{}", product.id)))
            .anchor(Anchor::BottomRight)
            .trigger(
                Button::new(button_id)
                    .secondary()
                    .w(px(36.))
                    .h(px(36.))
                    .icon(Icon::new(IconName::Info).with_size(px(18.))),
            )
            .content(move |_, _, _| knowledge_peek_card(product.clone(), muted, foreground))
    }

    fn render_cart_panel(
        &self,
        cx: &Context<Self>,
        totals: &CartTotals,
        customer: Option<Customer>,
    ) -> impl IntoElement {
        use crate::theme::touch;
        let addon = self.addon_product();
        let pending_offer = self.pending_offer();
        v_flex()
            .w(touch::CART_W)
            .flex_none()
            .h_full()
            // Let the column shrink with the window instead of sizing to its
            // content: unconstrained, the lines list keeps its content
            // height and its scroll hitbox spills over totals and Pay.
            .min_h_0()
            .overflow_hidden()
            .gap(px(12.))
            .p(px(16.))
            .bg(cx.theme().sidebar)
            .border_l_1()
            .border_color(cx.theme().border)
            .child(self.render_cart_header())
            .when_some(customer, |panel, customer| {
                panel.child(self.render_recognition(&customer, cx))
            })
            .child(self.render_cart_lines(cx))
            .when_some(pending_offer, |panel, offer| panel.child(self.render_offer_prompt(offer, cx)))
            .when_some(addon, |panel, product| panel.child(self.render_addon(product, cx)))
            .when(!self.cart.lines.is_empty(), |panel| {
                panel.child(self.render_cart_actions(totals, cx))
            })
            .child(self.render_total_block(cx, totals))
    }

    /// The customer picker, full width: walk-in by default, a tap away
    /// from attaching a member.
    fn render_cart_header(&self) -> impl IntoElement {
        div()
            .flex_none()
            .w_full()
            .child(
                Select::new(&self.customer_select)
                    .large()
                    .h(crate::theme::touch::TAP)
                    .rounded(crate::theme::touch::RADIUS),
            )
    }

    /// Customer recognition: who this is to the shop, from their real
    /// sales. A customer the win-back rules call lost gets the emerald
    /// "Back after N days" line, the moment the brand exists for.
    fn render_recognition(&self, customer: &Customer, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let seen = self
            .recognition
            .as_ref()
            .filter(|(id, _)| *id == customer.id)
            .map(|(_, seen)| *seen);
        let returning = seen.is_some_and(|seen| seen.returning_after_lost);
        let meta = seen.map(|seen| match seen.days_since {
            None => "First visit".to_string(),
            Some(days) => format!("Last visit {} · {}", days_ago(days), plural(seen.visits, "visit")),
        });
        v_flex()
            .flex_none()
            .gap(px(4.))
            .px(px(10.))
            .py(px(8.))
            .rounded(crate::theme::metrics::RADIUS_LG)
            .border_1()
            .map(|block| {
                if returning {
                    block.bg(theme.success.opacity(0.08)).border_color(theme.success.opacity(0.35))
                } else {
                    block.bg(theme.muted).border_color(theme.border)
                }
            })
            .child(
                h_flex()
                    .gap(px(8.))
                    .items_center()
                    .child(
                        Icon::new(IconName::Link)
                            .with_size(px(14.))
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::theme::metrics::FONT_EMPHASIS)
                            .font_weight(FontWeight::MEDIUM)
                            .child(SharedString::from(customer.name.clone())),
                    )
                    .child(
                        super::money_text(format!("{} pts", customer.points))
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground),
                    ),
            )
            .when_some(meta, |block, meta| {
                block.child(
                    div()
                        .pl(px(22.))
                        .text_size(crate::theme::metrics::FONT_FOOTNOTE)
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(meta)),
                )
            })
            .when_some(seen.filter(|_| returning).and_then(|seen| seen.days_since), |block, days| {
                block.child(
                    h_flex()
                        .gap(px(8.))
                        .items_center()
                        .pt(px(2.))
                        .text_color(theme.success)
                        .child(Icon::new(IconName::Hand).with_size(px(14.)).text_color(theme.success))
                        .child(
                            div()
                                .text_size(crate::theme::metrics::FONT_BODY)
                                .font_weight(FontWeight::MEDIUM)
                                .child(SharedString::from(format!("Back after {days} days"))),
                        ),
                )
            })
    }

    fn render_cart_lines(&self, cx: &Context<Self>) -> impl IntoElement {
        use crate::theme::touch;
        if self.cart.lines.is_empty() {
            return v_flex()
                .flex_grow(1.)
                .min_h_0()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .child(
                    Icon::new(IconName::ScanBarcode)
                        .with_size(px(40.))
                        .text_color(crate::theme::text_dim()),
                )
                .child(
                    div()
                        .text_size(touch::FONT_BODY)
                        .text_color(cx.theme().muted_foreground)
                        .child("Scan a barcode or tap a product."),
                )
                .child(
                    h_flex()
                        .gap(px(16.))
                        .text_size(px(13.))
                        .text_color(crate::theme::text_dim())
                        .child(key_hint("Enter", "adds a scan", cx.theme().border))
                        .child(key_hint("F2", "takes payment", cx.theme().border)),
                )
                .into_any_element();
        }
        let mut list = v_flex()
            .id("cart-lines")
            .flex_grow(1.)
            .min_h_0()
            .gap(px(8.))
            .overflow_y_scrollbar();
        for line in &self.cart.lines {
            list = list.child(self.render_cart_line(line, cx));
        }
        list.into_any_element()
    }

    /// One cart line: name and unit price, a big − qty + stepper (− at one
    /// removes the line), and the line total.
    fn render_cart_line(&self, line: &cart::CartLine, cx: &Context<Self>) -> impl IntoElement {
        use crate::theme::touch;
        let theme = cx.theme();
        let gross_minor = (line.unit_minor as f64 * line.qty).round() as i64;
        let hover = theme.secondary_hover;
        let step = |id: String, icon: IconName, qty: f64| {
            let line_id = line.id.clone();
            div()
                .id(SharedString::from(id))
                .cursor_pointer()
                .size(touch::TAP_SM)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(10.))
                .hover(move |style| style.bg(hover))
                .active(|style| style.opacity(0.7))
                .child(Icon::new(icon).with_size(px(22.)))
                .on_click(cx.listener(move |this, _, _, cx| this.set_line_qty(&line_id, qty, cx)))
        };
        h_flex()
            .flex_none()
            .gap(px(10.))
            .items_center()
            .p(px(10.))
            .pl(px(16.))
            .rounded(touch::RADIUS)
            .bg(theme.secondary)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(touch::FONT_ITEM)
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_clamp(2)
                            .child(SharedString::from(line.name.clone())),
                    )
                    .child(
                        super::money_text(fmt_money(line.unit_minor, self.currency))
                            .text_size(px(14.))
                            .text_color(theme.muted_foreground),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .rounded(px(10.))
                    .bg(theme.muted)
                    .child(step(format!("qty-dec-{}", line.id), IconName::Minus, line.qty - 1.0))
                    .child(
                        super::money_text(qty_label(line.qty))
                            .min_w(px(28.))
                            .text_center()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(step(format!("qty-inc-{}", line.id), IconName::Plus, line.qty + 1.0)),
            )
            .child(
                super::money_text(fmt_number(gross_minor, self.currency))
                    .flex_none()
                    .min_w(px(84.))
                    .text_right()
                    .text_size(touch::FONT_ITEM)
                    .font_weight(FontWeight::SEMIBOLD),
            )
    }

    /// Discount and Clear, the two whole-sale actions. Clear asks for a
    /// second tap (the button turns red and says so) so a stray finger
    /// cannot empty a full cart.
    fn render_cart_actions(&self, totals: &CartTotals, cx: &Context<Self>) -> impl IntoElement {
        use crate::theme::touch;
        let theme = cx.theme();
        let hover = theme.secondary_hover;
        let discount_label = match self.cart.order_discount {
            Some(Discount::Percent { bps }) if totals.discount_minor > 0 => {
                SharedString::from(format!("Discount {}%", percent_label(bps)))
            }
            _ => SharedString::from("Discount"),
        };
        let code_label = match &self.offer {
            Some(offer) => SharedString::from(offer.code.clone()),
            None => SharedString::from("Code"),
        };
        let has_code = self.offer.is_some();
        let armed = self.clear_armed;
        let danger = theme.danger;
        let action = |id: &'static str| {
            h_flex()
                .id(id)
                .cursor_pointer()
                .flex_1()
                .h(touch::TAP)
                .gap(px(8.))
                .items_center()
                .justify_center()
                .rounded(touch::RADIUS)
                .text_size(touch::FONT_LABEL)
                .font_weight(FontWeight::MEDIUM)
                .active(|style| style.opacity(0.75))
        };
        h_flex()
            .flex_none()
            .gap(px(8.))
            .child(
                action("cart-discount")
                    .bg(theme.secondary)
                    .hover(move |style| style.bg(hover))
                    .when(totals.discount_minor > 0, |button| {
                        button.border_1().border_color(theme.accent)
                    })
                    .child(Icon::new(IconName::Percent).with_size(px(18.)))
                    .child(discount_label)
                    .on_click(cx.listener(|this, _, window, cx| this.open_discount(window, cx))),
            )
            .child(
                action("cart-code")
                    .bg(theme.secondary)
                    .hover(move |style| style.bg(hover))
                    .when(has_code, |button| button.border_1().border_color(theme.accent))
                    .child(Icon::new(IconName::Ticket).with_size(px(18.)))
                    .child(code_label)
                    .when(!has_code, |button| button.child(super::key_cap("F3")))
                    .on_click(cx.listener(|this, _, window, cx| this.open_code(window, cx))),
            )
            .child(
                action("cart-clear")
                    .text_color(danger)
                    .map(|button| {
                        if armed {
                            button.bg(danger.opacity(0.18)).border_1().border_color(danger)
                        } else {
                            button.bg(theme.secondary).hover(move |style| style.bg(hover))
                        }
                    })
                    .child(Icon::new(IconName::Trash).with_size(px(18.)))
                    .child(if armed { "Tap again to clear" } else { "Clear" })
                    .on_click(cx.listener(|this, _, _, cx| this.tap_clear(cx))),
            )
    }

    /// Order discount: preset percentages as big buttons, a custom
    /// percentage, and "No discount".
    fn open_discount(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        const PRESETS: [i64; 6] = [5, 10, 15, 20, 25, 50];
        let register = cx.entity();
        let percent = self.percent.clone();
        let current = self.asked_bps;
        // Profit guard: presets past the limit are greyed out, and the
        // dialog says how far this cart can go.
        let max = self.max_discount_bps();
        let floor = self.settings.config.min_margin_pct;
        let limit_text = max.map(|max| {
            SharedString::from(if max == 0 {
                format!("No discount keeps your {floor}% minimum profit on this sale.")
            } else {
                format!("Up to {}% keeps at least {floor}% profit over HPP.", percent_label(max))
            })
        });
        window.open_dialog(cx, move |dialog, _, cx| {
            use crate::theme::touch;
            let theme = cx.theme();
            let (surface, hover, primary, on_primary) =
                (theme.secondary, theme.secondary_hover, theme.primary, theme.primary_foreground);
            let mut grid = div().grid().grid_cols(3).gap(px(10.));
            for pct in PRESETS {
                let register = register.clone();
                let selected = current == Some(pct * 100);
                let allowed = max.is_none_or(|max| pct * 100 <= max);
                grid = grid.child(
                    div()
                        .id(SharedString::from(format!("disc-{pct}")))
                        .cursor_pointer()
                        .h(px(72.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(touch::RADIUS)
                        .text_size(px(24.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .map(|key| {
                            if selected {
                                key.bg(primary).text_color(on_primary)
                            } else if !allowed {
                                key.bg(surface).opacity(0.35)
                            } else {
                                key.bg(surface).hover(move |style| style.bg(hover))
                            }
                        })
                        .child(SharedString::from(format!("{pct}%")))
                        .when(allowed, |key| {
                            key.on_click(move |_, window, cx| {
                                register.update(cx, |r, cx| r.set_order_percent(pct * 100, window, cx));
                                window.close_dialog(cx);
                            })
                        }),
                );
            }
            let apply = {
                let register = register.clone();
                move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
                    register.update(cx, |r, cx| r.apply_order_percent(cx));
                    window.close_dialog(cx);
                }
            };
            let remove = {
                let register = register.clone();
                move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
                    register.update(cx, |r, cx| r.clear_order_percent(window, cx));
                    window.close_dialog(cx);
                }
            };
            dialog
                .title("Discount on the whole sale")
                .w(px(520.))
                .child(
                    v_flex()
                        .gap(px(16.))
                        .when_some(limit_text.clone(), |col, text| {
                            col.child(div().text_size(touch::FONT_LABEL).text_color(crate::theme::text_dim()).child(text))
                        })
                        .child(grid)
                        .child(
                            h_flex()
                                .gap(px(10.))
                                .child(
                                    div()
                                        .flex_1()
                                        .h(touch::TAP)
                                        .px(px(14.))
                                        .flex()
                                        .items_center()
                                        .rounded(touch::RADIUS)
                                        .border_1()
                                        .border_color(crate::theme::border_strong())
                                        .child(
                                            Input::new(&percent)
                                                .id("disc-percent")
                                                .appearance(false)
                                                .text_size(px(18.)),
                                        ),
                                )
                                .child(
                                    Button::new("discount-apply")
                                        .primary()
                                        .h(touch::TAP)
                                        .w(px(120.))
                                        .label("Apply %")
                                        .on_click(apply),
                                ),
                        )
                        .child(
                            Button::new("discount-clear")
                                .ghost()
                                .h(touch::TAP)
                                .w_full()
                                .label("No discount")
                                .on_click(remove),
                        ),
                )
        });
    }

    /// Win-back code: the cashier types what the customer shows from
    /// WhatsApp; a valid code attaches the customer and takes its discount.
    pub fn open_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let register = cx.entity();
        let input = self.code_input.clone();
        let applied = self.offer.as_ref().map(|o| o.code.clone());
        window.open_dialog(cx, move |dialog, _, cx| {
            use crate::theme::touch;
            let theme = cx.theme();
            let apply = {
                let register = register.clone();
                let input = input.clone();
                move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
                    let raw = input.read(cx).value().to_string();
                    let result = register.update(cx, |r, cx| r.apply_code(&raw, window, cx));
                    match result {
                        Ok(()) => window.close_dialog(cx),
                        Err(message) => window.push_notification(Notification::error(message), cx),
                    }
                }
            };
            let remove = {
                let register = register.clone();
                move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
                    register.update(cx, |r, cx| {
                        r.offer = None;
                        r.clear_order_percent(window, cx);
                    });
                    window.close_dialog(cx);
                }
            };
            dialog
                .title("Win-back code")
                .w(px(520.))
                .child(
                    v_flex()
                        .gap(px(16.))
                        .child(
                            div()
                                .text_size(touch::FONT_BODY)
                                .text_color(theme.muted_foreground)
                                .child("The code from the customer's WhatsApp message. It attaches the customer and takes the discount."),
                        )
                        .child(
                            h_flex()
                                .gap(px(10.))
                                .child(
                                    div()
                                        .flex_1()
                                        .h(px(64.))
                                        .px(px(16.))
                                        .flex()
                                        .items_center()
                                        .rounded(touch::RADIUS)
                                        .border_1()
                                        .border_color(crate::theme::border_strong())
                                        .child(
                                            Input::new(&input)
                                                .id("code-input")
                                                .appearance(false)
                                                .text_size(px(26.)),
                                        ),
                                )
                                .child(Button::new("code-apply").primary().h(px(64.)).w(px(140.)).label("Apply").on_click(apply)),
                        )
                        .when_some(applied.clone(), |col, code| {
                            col.child(
                                Button::new("code-remove")
                                    .ghost()
                                    .h(touch::TAP)
                                    .w_full()
                                    .label(SharedString::from(format!("Remove code {code}")))
                                    .on_click(remove),
                            )
                        }),
                )
        });
        let input = self.code_input.clone();
        window.defer(cx, move |window, cx| input.update(cx, |i, cx| i.focus(window, cx)));
    }

    /// The add-on button: what people usually take with what was just
    /// added, one big tap to add it, or No.
    fn render_addon(&self, product: Product, cx: &Context<Self>) -> impl IntoElement {
        use crate::theme::touch;
        let theme = cx.theme();
        let hover = theme.secondary_hover;
        let product_id = product.id.clone();
        let anchor = self
            .addon_offers
            .iter()
            .rev()
            .find(|o| o.partner_id == product.id)
            .and_then(|o| self.products.iter().find(|p| p.id == o.anchor_id))
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let attach = self
            .addon_pairs
            .values()
            .find(|p| p.partner_id == product.id)
            .map(|p| p.attach_pct)
            .unwrap_or(0);
        v_flex()
            .flex_none()
            .gap(px(8.))
            .p(px(12.))
            .rounded(touch::RADIUS_LG)
            .bg(theme.background)
            .border_1()
            .border_color(theme.accent)
            .child(
                div()
                    .text_size(touch::FONT_LABEL)
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(format!("Ask: {attach}% of {anchor} orders take this too"))),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        h_flex()
                            .id("addon-add")
                            .cursor_pointer()
                            .flex_1()
                            .min_w_0()
                            .h(touch::TAP)
                            .px(px(14.))
                            .gap(px(10.))
                            .items_center()
                            .rounded(touch::RADIUS)
                            .bg(theme.primary)
                            .text_color(theme.primary_foreground)
                            .active(|style| style.opacity(0.8))
                            .child(Icon::new(IconName::Plus).with_size(px(20.)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(17.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(SharedString::from(product.name.clone())),
                            )
                            .child(
                                super::money_text(fmt_money(product.price_minor, self.currency))
                                    .text_size(px(17.))
                                    .font_weight(FontWeight::SEMIBOLD),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(product) = this.products.iter().find(|p| p.id == product_id).cloned() {
                                    this.add_product(&product, window, cx);
                                }
                            })),
                    )
                    .child(
                        h_flex()
                            .id("addon-no")
                            .cursor_pointer()
                            .flex_none()
                            .w(px(76.))
                            .h(touch::TAP)
                            .items_center()
                            .justify_center()
                            .rounded(touch::RADIUS)
                            .bg(theme.secondary)
                            .hover(move |style| style.bg(hover))
                            .text_size(touch::FONT_LABEL)
                            .font_weight(FontWeight::MEDIUM)
                            .child("No")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.addon_prompt = None;
                                cx.notify();
                            })),
                    ),
            )
    }

    /// The attached customer has a win-back code waiting: one tap applies it.
    fn render_offer_prompt(&self, offer: WinbackOfferRow, cx: &Context<Self>) -> impl IntoElement {
        use crate::theme::touch;
        let theme = cx.theme();
        let code = offer.code.clone();
        h_flex()
            .id("offer-apply")
            .cursor_pointer()
            .flex_none()
            .h(touch::TAP)
            .px(px(14.))
            .gap(px(10.))
            .items_center()
            .rounded(touch::RADIUS)
            .border_1()
            .border_color(theme.accent)
            .bg(theme.background)
            .active(|style| style.opacity(0.8))
            .child(Icon::new(IconName::Ticket).with_size(px(18.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(touch::FONT_LABEL)
                    .child(SharedString::from(format!("Win-back code {} · {}% off", offer.code, offer.pct))),
            )
            .child(div().text_size(touch::FONT_LABEL).font_weight(FontWeight::SEMIBOLD).child("Apply"))
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Err(message) = this.apply_code(&code, window, cx) {
                    window.push_notification(Notification::error(message), cx);
                }
            }))
    }

    /// The total block: the sale's money and its one action, the same
    /// layout whether or not the sale is live so nothing jumps when the
    /// first item lands.
    fn render_total_block(&self, cx: &Context<Self>, totals: &CartTotals) -> impl IntoElement {
        use crate::theme::touch;
        let theme = cx.theme();
        let has_items = !self.cart.lines.is_empty();
        let has_shift = self.active_shift.is_some();
        let live = has_items && has_shift;
        let (strong, soft) = (theme.foreground, theme.muted_foreground);
        let units: f64 = self.cart.lines.iter().map(|line| line.qty).sum();
        let row = |label: String, value: String| {
            h_flex()
                .justify_between()
                .text_size(touch::FONT_LABEL)
                .text_color(soft)
                .child(label)
                .child(super::money_text(value).text_right())
        };
        let note = if has_shift {
            self.guard_note()
        } else {
            Some(SharedString::from("No open shift. Open one to take payment."))
        };
        v_flex()
            .id("total-block")
            .flex_none()
            .gap(px(4.))
            .child(row(
                format!("Subtotal · {}", plural_units(units)),
                fmt_money(totals.gross_minor, self.currency),
            ))
            .when(totals.discount_minor > 0, |block| {
                block.child(row(
                    "Discount".to_string(),
                    format!("-{}", fmt_money(totals.discount_minor, self.currency)),
                ))
            })
            .child(row("Tax".to_string(), fmt_money(totals.tax_minor, self.currency)))
            .child(
                h_flex()
                    .mt(px(4.))
                    .justify_between()
                    .items_end()
                    .gap_2()
                    .child(
                        div()
                            .pb(px(8.))
                            .text_size(touch::FONT_BODY)
                            .text_color(soft)
                            .child("Total"),
                    )
                    .child(
                        super::money_text(fmt_money(totals.total_minor, self.currency))
                            .text_right()
                            .text_size(touch::FONT_TOTAL)
                            .line_height(touch::FONT_TOTAL * 1.15)
                            .font_weight(FontWeight::BOLD)
                            .text_color(if has_items { strong } else { soft }),
                    ),
            )
            .when_some(note, |block, note| {
                block.child(
                    div()
                        .text_size(px(14.))
                        .text_color(theme.warning)
                        .child(note),
                )
            })
            .child(
                div()
                    .pt(px(10.))
                    .child(self.render_total_action(cx, live, has_shift, totals.total_minor)),
            )
    }

    /// The column's one primary button. Pay shows the amount it will take;
    /// with no shift it becomes Open shift; with an empty cart it is a
    /// visibly locked Pay.
    fn render_total_action(
        &self,
        cx: &Context<Self>,
        live: bool,
        has_shift: bool,
        total_minor: i64,
    ) -> impl IntoElement {
        use crate::theme::{metrics, touch};
        let theme = cx.theme();
        let label = if !has_shift {
            SharedString::from("Open shift")
        } else if live {
            SharedString::from(format!("Pay {}", fmt_money(total_minor, self.currency)))
        } else {
            SharedString::from("Pay")
        };
        let key = if has_shift { "F2" } else { "F7" };
        let enabled = live || !has_shift;
        let (fg, key_border) = if enabled {
            (theme.accent_foreground, theme.accent_foreground.opacity(0.3))
        } else {
            (crate::theme::text_dim(), theme.border)
        };
        let (accent, hover, active) = (theme.primary, theme.primary_hover, theme.primary_active);
        h_flex()
            .id(if has_shift { "pay" } else { "open-shift" })
            .w_full()
            .h(touch::TAP_XL)
            .gap(px(14.))
            .items_center()
            .justify_center()
            .rounded(touch::RADIUS_LG)
            .text_color(fg)
            .map(|button| {
                if enabled {
                    button
                        .cursor_pointer()
                        .bg(accent)
                        .hover(move |style| style.bg(hover))
                        .active(move |style| style.bg(active))
                } else {
                    button.border_1().border_color(theme.border)
                }
            })
            .when(live, |button| {
                button.on_click(cx.listener(|this, _, window, cx| this.open_pay(window, cx)))
            })
            .when(!has_shift, |button| {
                button.on_click(cx.listener(|_, _, _, cx| cx.emit(RegisterEvent::OpenShift)))
            })
            .child(
                super::money_text(label)
                    .text_size(touch::FONT_ACTION)
                    .font_weight(FontWeight::BOLD),
            )
            .child(
                div()
                    .px(px(6.))
                    .rounded(metrics::RADIUS_SM)
                    .border_1()
                    .border_color(key_border)
                    .font_family(crate::theme::FONT_MONO)
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(key),
            )
    }
}

impl Render for RegisterScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A menu pull landed while we were idle: reload the snapshot here,
        // in this view's own render pass (see spawn_menu_watch).
        self.consume_pending_menu_revision(cx);
        // Customers' "Sell to" hands its pick over through the
        // SelectedCustomer global; consume it before reading the cart.
        self.consume_selected_customer(window, cx);
        self.apply_guard();
        self.sync_recognition();
        self.sync_offer_hint();
        let totals = self.totals();
        let products = self.filtered_products(cx);
        let customer = self.attached_customer();
        // As many columns as keep every tile at least TILE_MIN_W wide.
        use crate::theme::touch;
        let catalog_w = f32::from(window.viewport_size().width)
            - f32::from(touch::RAIL_W)
            - f32::from(touch::CART_W)
            - 32.;
        let columns = ((catalog_w + 12.) / (touch::TILE_MIN_W + 12.)).floor().clamp(2., 6.) as u16;

        h_flex()
            .size_full()
            .items_stretch()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .p(px(16.))
                    .child(self.render_catalog(cx, &products, columns)),
            )
            .child(self.render_cart_panel(cx, &totals, customer))
    }
}

// ── Pay dialog ────────────────────────────────────────────────────────────

/// What the pay dialog reports back to the register (and the training
/// sandbox, which reuses this dialog verbatim).
pub(crate) enum PayEvent {
    Paid {
        tenders: Vec<Tender>,
        redeem_points: i64,
        /// WhatsApp number typed at payment (no customer was attached).
        phone: Option<String>,
    },
}

impl EventEmitter<PayEvent> for PayDialog {}

#[derive(Clone)]
pub(crate) struct CustomerSnapshot {
    // Kept so the paid receipt can address the buyer; only `points`
    // feeds the pay dialog today.
    #[allow(dead_code)]
    pub(crate) id: String,
    #[allow(dead_code)]
    pub(crate) name: String,
    pub(crate) points: i64,
}

/// Payment screen: tender composition against the running total, sized
/// for touch. Pick a method (Cash by default), type or tap the amount, then
/// Complete. An amount short of the total stages a partial tender and the
/// screen stays open for the rest (split payment). Cash may overpay (that
/// is what change is); the other methods cannot; loyalty points ride in as
/// their own tender. `pub(crate)`: the training screen reuses it verbatim.
pub(crate) struct PayDialog {
    total_minor: i64,
    customer: Option<CustomerSnapshot>,
    loyalty: LoyaltyConfig,
    currency: &'static Currency,
    tenders: Vec<Tender>,
    method: TenderMethod,
    entry: Entity<InputState>,
    redeem: Entity<InputState>,
    /// Optional WhatsApp number, shown when the store asks for one and no
    /// customer is attached (never in training).
    phone: Entity<InputState>,
    ask_phone: bool,
    /// Paid was emitted: ignore further commits (Enter reaches both the
    /// amount field and the dialog's Confirm) until the next reset.
    settled: bool,
    _subscriptions: Vec<Subscription>,
}

/// Opens the payment screen as a near-full-window dialog.
pub(crate) fn open_pay_dialog(pay: Entity<PayDialog>, window: &mut Window, cx: &mut gpui_kit::App) {
    let width = (f32::from(window.viewport_size().width) - 64.).clamp(900., 1240.);
    let surface = cx.theme().background;
    let content = pay.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        let confirm = content.clone();
        dialog
            .w(px(width))
            .margin_top(px(24.))
            .close_button(false)
            .bg(surface)
            // Enter is the dialog's Confirm: complete (or stage) instead of
            // closing. The register closes the dialog once the sale settles.
            .on_ok(move |_, window, cx| {
                confirm.update(cx, |pay, cx| pay.commit(window, cx));
                false
            })
            .child(content.clone())
    });
    // The dialog takes focus as it opens; hand it to the amount field so a
    // cashier can type the cash handed over and press Enter.
    window.defer(cx, move |window, cx| {
        pay.update(cx, |pay, cx| pay.focus_entry(window, cx));
    });
}

const METHODS: [(TenderMethod, &str, IconName); 4] = [
    (TenderMethod::Cash, "Cash", IconName::Banknote),
    (TenderMethod::Card, "Card", IconName::CreditCard),
    (TenderMethod::Qris, "QRIS", IconName::QrCode),
    (TenderMethod::Transfer, "Transfer", IconName::Landmark),
];

fn method_label(method: TenderMethod) -> &'static str {
    match method {
        TenderMethod::Cash => "Cash",
        TenderMethod::Card => "Card",
        TenderMethod::Qris => "QRIS",
        TenderMethod::Transfer => "Transfer",
        TenderMethod::Loyalty => "Points",
    }
}

/// Cash a customer is likely to hand over for `remaining`: the next round
/// amounts above it (Rp 15.000 -> 16.000, 20.000, 50.000), padded with
/// the next Rp 50.000 steps. Amounts are in the currency's minor units.
pub(crate) fn quick_cash(remaining: i64) -> Vec<i64> {
    const STEPS: [i64; 6] = [2_000, 5_000, 10_000, 20_000, 50_000, 100_000];
    if remaining <= 0 {
        return Vec::new();
    }
    let mut out: Vec<i64> = STEPS
        .iter()
        .map(|step| (remaining + step - 1) / step * step)
        .filter(|amount| *amount > remaining)
        .collect();
    out.sort_unstable();
    out.dedup();
    out.truncate(3);
    while out.len() < 3 {
        let top = out.last().copied().unwrap_or(remaining);
        out.push((top / 50_000 + 1) * 50_000);
    }
    out
}

impl PayDialog {
    pub(crate) fn new(
        currency: &'static Currency,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let entry = cx.new(|cx| InputState::new(window, cx));
        let redeem = cx.new(|cx| InputState::new(window, cx).placeholder("Points"));
        let phone = cx.new(|cx| InputState::new(window, cx).placeholder("08…"));
        // Enter in the amount completes, like the big button.
        let subscription = cx.subscribe_in(&entry, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.commit(window, cx);
            }
        });
        Self {
            total_minor: 0,
            customer: None,
            loyalty: LoyaltyConfig {
                enabled: false,
                step_minor: 0,
                points_per_step: 0,
                point_value_minor: 0,
            },
            currency,
            tenders: Vec::new(),
            method: TenderMethod::Cash,
            entry,
            redeem,
            phone,
            ask_phone: false,
            settled: false,
            _subscriptions: vec![subscription],
        }
    }

    pub(crate) fn reset(
        &mut self,
        total_minor: i64,
        customer: Option<CustomerSnapshot>,
        loyalty: LoyaltyConfig,
        currency: &'static Currency,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.total_minor = total_minor;
        self.customer = customer;
        self.loyalty = loyalty;
        self.currency = currency;
        self.tenders.clear();
        self.method = TenderMethod::Cash;
        self.settled = false;
        self.entry
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.redeem
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.phone
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.ask_phone = false;
        cx.notify();
    }

    /// Show the WhatsApp number field (the register calls this after reset).
    pub(crate) fn set_ask_phone(&mut self, ask: bool, cx: &mut Context<Self>) {
        self.ask_phone = ask && self.customer.is_none();
        cx.notify();
    }

    pub(crate) fn unsettle(&mut self) {
        self.settled = false;
    }

    pub(crate) fn focus_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.entry.update(cx, |input, cx| input.focus(window, cx));
    }

    fn entry_minor(&self, cx: &gpui_kit::App) -> i64 {
        digits_of(&self.entry.read(cx).value()).parse().unwrap_or(0)
    }

    fn clear_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.entry
            .update(cx, |input, cx| input.set_value("", window, cx));
    }

    fn max_redeem(&self) -> i64 {
        self.customer
            .as_ref()
            .map(|customer| max_redeemable_points(customer.points, self.total_minor, &self.loyalty))
            .unwrap_or(0)
    }

    fn clamped_redeem_points(&self, max_redeem: i64, cx: &gpui_kit::App) -> i64 {
        digits_of(&self.redeem.read(cx).value())
            .parse::<i64>()
            .unwrap_or(0)
            .clamp(0, max_redeem)
    }

    /// The payment as it stands, counting staged loyalty points.
    fn payment(&self, cx: &gpui_kit::App) -> PaymentState {
        let points = self.clamped_redeem_points(self.max_redeem(), cx);
        let value = redeem_value_minor(points, &self.loyalty);
        let mut tenders = self.tenders.clone();
        if value > 0 {
            tenders.push(payments::new_tender(TenderMethod::Loyalty, value, None));
        }
        payments::evaluate_payments(self.total_minor, &tenders)
    }

    /// Stage `amount` with the selected method. Cash records what was handed
    /// over (unclamped); the rest cannot exceed the balance.
    fn stage(&mut self, amount: i64, remaining: i64) {
        let applied = amount.min(remaining);
        if applied <= 0 {
            return;
        }
        let given = (self.method == TenderMethod::Cash).then_some(amount);
        self.tenders
            .push(payments::new_tender(self.method, applied, given));
    }

    /// The big button (and Enter): stage the typed amount, or the exact
    /// balance when nothing is typed, then finish if that covers the total.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settled {
            return;
        }
        let remaining = self.payment(cx).remaining_minor.max(0);
        let entry = self.entry_minor(cx);
        if entry > 0 {
            self.stage(entry, remaining);
            self.clear_entry(window, cx);
        } else if remaining > 0 {
            self.stage(remaining, remaining);
        }
        if self.payment(cx).complete {
            self.settled = true;
            let redeem_points = self.clamped_redeem_points(self.max_redeem(), cx);
            let phone = self.ask_phone.then(|| self.phone.read(cx).value().trim().to_string()).filter(|p| !p.is_empty());
            cx.emit(PayEvent::Paid {
                tenders: self.tenders.clone(),
                redeem_points,
                phone,
            });
        }
        cx.notify();
    }

    /// A quick-cash button: the customer handed over exactly this.
    fn quick(&mut self, amount: i64, window: &mut Window, cx: &mut Context<Self>) {
        let remaining = self.payment(cx).remaining_minor.max(0);
        self.stage(amount, remaining);
        self.clear_entry(window, cx);
        cx.notify();
    }

    fn push_entry_digits(&mut self, digits: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut value = digits_of(&self.entry.read(cx).value());
        value.push_str(digits);
        let trimmed = value.trim_start_matches('0');
        let next = if trimmed.is_empty() { "0" } else { trimmed };
        self.entry
            .update(cx, |input, cx| input.set_value(next, window, cx));
    }

    fn backspace_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut value = digits_of(&self.entry.read(cx).value());
        value.pop();
        self.entry
            .update(cx, |input, cx| input.set_value(value, window, cx));
    }

    // ── Left column: what is owed ────────────────────────────────────────

    fn render_due(&self, payment: &PaymentState, cx: &Context<Self>) -> impl IntoElement {
        use crate::theme::touch;
        let theme = cx.theme();
        let label = |text: &'static str| {
            div()
                .font_family(crate::theme::FONT_MONO)
                .text_size(px(13.))
                .text_color(crate::theme::text_dim())
                .child(text)
        };
        let max_redeem = self.max_redeem();
        let staged_points = self.clamped_redeem_points(max_redeem, cx);
        let mut staged = v_flex().gap(px(4.));
        for tender in &self.tenders {
            let tender_id = tender.id.clone();
            let hover = theme.secondary_hover;
            let name = match tender.given_minor {
                Some(given) if given > tender.applied_minor => format!(
                    "{} · given {}",
                    method_label(tender.method),
                    fmt_money(given, self.currency)
                ),
                _ => method_label(tender.method).to_string(),
            };
            staged = staged.child(
                h_flex()
                    .gap(px(10.))
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(touch::FONT_BODY)
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(name)),
                    )
                    .child(
                        super::money_text(fmt_money(tender.applied_minor, self.currency))
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("tender-remove-{}", tender.id)))
                            .cursor_pointer()
                            .size(px(44.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(10.))
                            .hover(move |style| style.bg(hover))
                            .child(Icon::new(IconName::Close).with_size(px(18.)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.tenders.retain(|t| t.id != tender_id);
                                cx.notify();
                            })),
                    ),
            );
        }
        let (status_label, status_value, status_color) = if payment.complete {
            ("CHANGE", payment.change_minor, theme.success)
        } else {
            ("REMAINING", payment.remaining_minor.max(0), theme.foreground)
        };
        v_flex()
            .w(px(400.))
            .flex_none()
            .gap(px(16.))
            .p(px(28.))
            .border_r_1()
            .border_color(theme.border)
            .child(label("AMOUNT DUE"))
            .child(
                super::money_text(fmt_money(self.total_minor, self.currency))
                    .text_size(touch::FONT_DUE)
                    .line_height(touch::FONT_DUE * 1.1)
                    .font_weight(FontWeight::BOLD),
            )
            .when(!self.tenders.is_empty(), |col| {
                col.child(
                    v_flex()
                        .gap(px(8.))
                        .p(px(16.))
                        .rounded(touch::RADIUS_LG)
                        .bg(theme.secondary)
                        .child(label("PAID SO FAR"))
                        .child(staged),
                )
            })
            .child(
                v_flex()
                    .gap(px(6.))
                    .p(px(16.))
                    .rounded(touch::RADIUS_LG)
                    .bg(theme.secondary)
                    .child(label(status_label))
                    .child(
                        super::money_text(fmt_money(status_value, self.currency))
                            .text_size(px(40.))
                            .line_height(px(46.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(status_color),
                    ),
            )
            .when(max_redeem > 0, |col| {
                col.child(
                    v_flex()
                        .gap(px(8.))
                        .child(label("REDEEM POINTS"))
                        .child(
                            h_flex()
                                .gap(px(10.))
                                .items_center()
                                .child(
                                    div()
                                        .w(px(140.))
                                        .h(touch::TAP_SM)
                                        .px(px(12.))
                                        .flex()
                                        .items_center()
                                        .rounded(touch::RADIUS)
                                        .border_1()
                                        .border_color(crate::theme::border_strong())
                                        .child(
                                            Input::new(&self.redeem)
                                                .id("pay-redeem")
                                                .appearance(false)
                                                .text_size(px(17.)),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .text_color(theme.muted_foreground)
                                        .child(SharedString::from(format!(
                                            "max {max_redeem} · = {}",
                                            fmt_money(
                                                redeem_value_minor(staged_points, &self.loyalty),
                                                self.currency
                                            )
                                        ))),
                                ),
                        ),
                )
            })
            .when(self.ask_phone, |col| {
                col.child(
                    v_flex()
                        .gap(px(8.))
                        .child(label("WHATSAPP NUMBER · OPTIONAL"))
                        .child(
                            div()
                                .h(touch::TAP_SM)
                                .px(px(12.))
                                .flex()
                                .items_center()
                                .rounded(touch::RADIUS)
                                .border_1()
                                .border_color(crate::theme::border_strong())
                                .child(Input::new(&self.phone).id("pay-phone").appearance(false).text_size(px(17.))),
                        )
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(theme.muted_foreground)
                                .child("For points and a receipt on WhatsApp."),
                        ),
                )
            })
            .child(div().flex_1())
            .child(
                h_flex()
                    .id("pay-cancel")
                    .cursor_pointer()
                    .h(px(64.))
                    .gap(px(10.))
                    .items_center()
                    .justify_center()
                    .rounded(touch::RADIUS_LG)
                    .border_1()
                    .border_color(crate::theme::border_strong())
                    .text_size(px(18.))
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|style| style.bg(gpui_kit::white().opacity(0.04)))
                    .child(Icon::new(IconName::ArrowLeft).with_size(px(20.)))
                    .child("Back to cart")
                    .child(super::key_cap("Esc"))
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            )
    }

    // ── Right column: how it is paid ─────────────────────────────────────

    fn render_methods(&self, cx: &Context<Self>) -> impl IntoElement {
        use crate::theme::touch;
        let theme = cx.theme();
        let mut row = div().grid().grid_cols(4).gap(px(10.));
        for (method, label, icon) in METHODS {
            let selected = self.method == method;
            let hover = theme.secondary_hover;
            row = row.child(
                v_flex()
                    .id(SharedString::from(format!("method-{}", method.as_str())))
                    .cursor_pointer()
                    .h(px(76.))
                    .gap(px(6.))
                    .items_center()
                    .justify_center()
                    .rounded(touch::RADIUS_LG)
                    .text_size(touch::FONT_BODY)
                    .font_weight(FontWeight::SEMIBOLD)
                    .map(|tile| {
                        if selected {
                            tile.bg(theme.primary).text_color(theme.primary_foreground)
                        } else {
                            tile.bg(theme.secondary).hover(move |style| style.bg(hover))
                        }
                    })
                    .child(Icon::new(icon).with_size(px(24.)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.method = method;
                        cx.notify();
                    })),
            );
        }
        row
    }

    fn render_quick(&self, remaining: i64, cx: &Context<Self>) -> impl IntoElement {
        use crate::theme::touch;
        let theme = cx.theme();
        let hover = theme.secondary_hover;
        let key = |id: SharedString, label: SharedString| {
            div()
                .id(id)
                .cursor_pointer()
                .h(px(64.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(touch::RADIUS)
                .font_family(crate::theme::FONT_MONEY)
                .font_features(crate::theme::tabular())
                .text_size(px(18.))
                .font_weight(FontWeight::SEMIBOLD)
                .hover(move |style| style.bg(hover))
                .active(|style| style.opacity(0.75))
                .child(label)
        };
        let mut row = div().grid().grid_cols(4).gap(px(10.)).child(
            key("quick-exact".into(), "Exact".into())
                .bg(theme.muted)
                .on_click(cx.listener(move |this, _, window, cx| this.quick(remaining, window, cx))),
        );
        if self.method == TenderMethod::Cash {
            for amount in quick_cash(remaining) {
                row = row.child(
                    key(
                        SharedString::from(format!("cash-{amount}")),
                        SharedString::from(fmt_money(amount, self.currency)),
                    )
                    .border_1()
                    .border_color(crate::theme::border_strong())
                    .on_click(cx.listener(move |this, _, window, cx| this.quick(amount, window, cx))),
                );
            }
        }
        row
    }

    fn render_entry(&self, cx: &Context<Self>) -> impl IntoElement {
        use crate::theme::touch;
        let theme = cx.theme();
        let label = match self.method {
            TenderMethod::Cash => "Cash received",
            _ => "Amount",
        };
        h_flex()
            .h(px(72.))
            .px(px(22.))
            .gap(px(16.))
            .items_center()
            .rounded(touch::RADIUS_LG)
            .bg(theme.secondary)
            .border_1()
            .border_color(crate::theme::border_strong())
            .child(
                div()
                    .flex_none()
                    .text_size(touch::FONT_LABEL)
                    .text_color(crate::theme::text_dim())
                    .child(label),
            )
            .child(
                Input::new(&self.entry)
                    .id("pay-entry")
                    .appearance(false)
                    .flex_1()
                    .h(px(48.))
                    .text_size(px(30.))
                    .line_height(px(40.)),
            )
    }

    fn render_pad(&self, cx: &Context<Self>) -> impl IntoElement {
        use crate::theme::touch;
        const KEYS: [&str; 12] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "00", "0", "⌫"];
        let theme = cx.theme();
        let hover = theme.secondary_hover;
        let mut grid = div().grid().grid_cols(3).gap(px(10.));
        for key in KEYS {
            grid = grid.child(
                div()
                    .id(SharedString::from(format!("num-{key}")))
                    .cursor_pointer()
                    .h(px(64.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(touch::RADIUS)
                    .bg(theme.secondary)
                    .hover(move |style| style.bg(hover))
                    .active(|style| style.opacity(0.7))
                    .text_size(px(26.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .map(|cell| {
                        if key == "⌫" {
                            cell.child(Icon::new(IconName::Delete).with_size(px(26.)))
                        } else {
                            cell.child(key)
                        }
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if key == "⌫" {
                            this.backspace_entry(window, cx);
                        } else {
                            this.push_entry_digits(key, window, cx);
                        }
                    })),
            );
        }
        grid
    }

    /// The one primary button, labelled with what it will do.
    fn render_commit(&self, payment: &PaymentState, cx: &Context<Self>) -> impl IntoElement {
        use crate::theme::touch;
        let theme = cx.theme();
        let remaining = payment.remaining_minor.max(0);
        let entry = self.entry_minor(cx);
        let method = method_label(self.method);
        let label = if payment.complete {
            if payment.change_minor > 0 {
                format!("Complete sale · change {}", fmt_money(payment.change_minor, self.currency))
            } else {
                "Complete sale".to_string()
            }
        } else if entry == 0 {
            format!("Charge {} by {method}", fmt_money(remaining, self.currency))
        } else if self.method == TenderMethod::Cash && entry >= remaining {
            format!("Complete sale · change {}", fmt_money(entry - remaining, self.currency))
        } else if entry >= remaining {
            format!("Charge {} by {method}", fmt_money(remaining, self.currency))
        } else {
            format!(
                "Add {} {method} · {} left",
                fmt_money(entry, self.currency),
                fmt_money(remaining - entry, self.currency)
            )
        };
        let (bg, hover, active, fg) =
            (theme.primary, theme.primary_hover, theme.primary_active, theme.primary_foreground);
        h_flex()
            .id("pay-paid")
            .cursor_pointer()
            .h(touch::TAP_XL)
            .gap(px(14.))
            .items_center()
            .justify_center()
            .rounded(touch::RADIUS_LG)
            .bg(bg)
            .hover(move |style| style.bg(hover))
            .active(move |style| style.bg(active))
            .text_color(fg)
            .child(
                super::money_text(label)
                    .text_size(touch::FONT_ACTION)
                    .font_weight(FontWeight::BOLD),
            )
            .child(
                div()
                    .px(px(6.))
                    .rounded(crate::theme::metrics::RADIUS_SM)
                    .border_1()
                    .border_color(fg.opacity(0.3))
                    .font_family(crate::theme::FONT_MONO)
                    .text_size(px(12.))
                    .child("Enter"),
            )
            .on_click(cx.listener(|this, _, window, cx| this.commit(window, cx)))
    }
}

impl Render for PayDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let payment = self.payment(cx);
        let remaining = payment.remaining_minor.max(0);
        h_flex()
            .items_stretch()
            .min_h(px(640.))
            .child(self.render_due(&payment, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(12.))
                    .p(px(28.))
                    .child(self.render_methods(cx))
                    .child(self.render_quick(remaining, cx))
                    .child(self.render_entry(cx))
                    .child(self.render_pad(cx))
                    .child(self.render_commit(&payment, cx)),
            )
    }
}

// ── Receipt dialog ────────────────────────────────────────────────────────

/// What the receipt dialog reports back: the reader dismissed the paper
/// ("New sale"). The register ignores it; the training receipt lesson
/// completes on exactly this seam.
pub(crate) enum ReceiptEvent {
    Closed,
}

impl EventEmitter<ReceiptEvent> for ReceiptDialog {}

/// Thermal-receipt preview for the last settled sale, with exports to disk
/// next to the ledger: `.txt` (rendered text) and `.bin` (ESC/POS bytes).
pub(crate) struct ReceiptDialog {
    sale: Option<Sale>,
    store: StoreInfo,
    currency: &'static Currency,
    receipts_dir: Option<PathBuf>,
    /// The customer's WhatsApp number (wa.me form), when the sale has one.
    whatsapp: Option<String>,
}

impl ReceiptDialog {
    pub(crate) fn new(receipts_dir: Option<PathBuf>, currency: &'static Currency) -> Self {
        Self {
            sale: None,
            store: StoreInfo::named("Kaseer"),
            currency,
            receipts_dir,
            whatsapp: None,
        }
    }

    pub(crate) fn show(&mut self, sale: Sale, store: StoreInfo) {
        self.sale = Some(sale);
        self.store = store;
        self.whatsapp = None;
    }

    /// Offer "Send on WhatsApp" for this receipt.
    pub(crate) fn set_whatsapp(&mut self, phone: Option<String>) {
        self.whatsapp = phone;
    }

    /// Drop the shown sale (a fresh training run starts clean).
    pub(crate) fn clear(&mut self) {
        self.sale = None;
    }

    /// The shown sale, if any: the training screen re-opens the paper for
    /// its receipt lesson.
    pub(crate) fn sale(&self) -> Option<Sale> {
        self.sale.clone()
    }

    fn write_receipt(
        &self,
        ext: &str,
        bytes: &[u8],
        ok_prefix: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let outcome = self
            .receipts_dir
            .as_deref()
            .ok_or_else(|| "cannot resolve the receipt folder".to_string())
            .and_then(|dir| {
                std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
                let path = dir.join(format!(
                    "{}.{ext}",
                    self.sale.as_ref().expect("dialog opens with a sale").number
                ));
                std::fs::write(&path, bytes)
                    .map(|_| path)
                    .map_err(|error| error.to_string())
            });
        match outcome {
            Ok(path) => window.push_notification(
                Notification::success(format!("{ok_prefix} {}", path.display())),
                cx,
            ),
            Err(message) => window.push_notification(Notification::error(message), cx),
        }
    }

    fn render_stat(
        &self,
        label: &str,
        value: String,
        color: Hsla,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .gap_0p5()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(label)),
            )
            .child(
                div()
                    .font_family(crate::theme::FONT_MONEY)
                            .font_features(crate::theme::tabular())
                    .text_base()
                    .font_semibold()
                    .text_color(color)
                    .child(SharedString::from(value)),
            )
    }
}

impl Render for ReceiptDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(sale) = self.sale.clone() else {
            return div().child("");
        };
        let text = render_receipt_text(&sale, &self.store, self.currency, 42);

        // The web `.receipt`: white paper, near-black ink, mono 12px,
        // 8px radius, 340px column.
        let mut paper = v_flex()
            .flex_grow(1.)
            .min_w_0()
            .gap_0p5()
            .rounded(px(8.))
            .bg(crate::theme::receipt_paper())
            .text_color(crate::theme::receipt_ink())
            .p(px(14.));
        for line in text.lines() {
            paper = paper.child(
                div()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(12.))
                    .whitespace_nowrap()
                    .child(SharedString::from(line.to_string())),
            );
        }

        let mut side = v_flex().w(px(150.)).gap_3().justify_center();
        side = side.child(self.render_stat(
            "CHANGE DUE",
            fmt_money(sale.change_minor, self.currency),
            cx.theme().success,
            cx,
        ));
        if sale.points_earned > 0 {
            side = side.child(self.render_stat(
                "POINTS EARNED",
                format!("+{}", sale.points_earned),
                cx.theme().foreground,
                cx,
            ));
        }
        if sale.points_redeemed > 0 {
            side = side.child(self.render_stat(
                "POINTS USED",
                format!("-{}", sale.points_redeemed),
                cx.theme().accent,
                cx,
            ));
        }

        v_flex()
            .gap_3()
            .when_some(sale.customer_name_snapshot.clone(), |dialog, name| {
                dialog.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(format!("Customer: {name}"))),
                )
            })
            .child(h_flex().gap_4().items_stretch().child(paper).child(side))
            .child(
                h_flex()
                    .gap(px(10.))
                    .child(
                        Button::new("receipt-export")
                            .secondary()
                            .h(crate::theme::touch::TAP)
                            .px(px(20.))
                            .label("Export .txt")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let Some(sale) = this.sale.clone() else {
                                    return;
                                };
                                let text =
                                    render_receipt_text(&sale, &this.store, this.currency, 42);
                                this.write_receipt(
                                    "txt",
                                    text.as_bytes(),
                                    "Receipt saved to",
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new("receipt-print-bytes")
                            .secondary()
                            .h(crate::theme::touch::TAP)
                            .px(px(20.))
                            .label("Print bytes")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let Some(sale) = this.sale.clone() else {
                                    return;
                                };
                                let text =
                                    render_receipt_text(&sale, &this.store, this.currency, 42);
                                let bytes = escpos_bytes(&text);
                                this.write_receipt(
                                    "bin",
                                    &bytes,
                                    "ESC/POS bytes written to",
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .when_some(self.whatsapp.clone(), |row, phone| {
                        row.child(
                            Button::new("receipt-whatsapp")
                                .secondary()
                                .h(crate::theme::touch::TAP)
                                .px(px(20.))
                                .icon(Icon::new(IconName::Send).with_size(px(16.)))
                                .label("WhatsApp")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let Some(sale) = this.sale.as_ref() else {
                                        return;
                                    };
                                    let text = receipt_text(sale, &this.store.name, this.currency);
                                    cx.open_url(&whatsapp_url(&phone, &text));
                                })),
                        )
                    })
                    .child(
                        Button::new("receipt-new-sale")
                            .primary()
                            .flex_1()
                            .h(crate::theme::touch::TAP)
                            .text_size(px(18.))
                            .label("New sale")
                            .on_click(cx.listener(|_, _, window, cx| {
                                cx.emit(ReceiptEvent::Closed);
                                window.close_dialog(cx);
                            })),
                    ),
            )
    }
}

// ── Select items ──────────────────────────────────────────────────────────

/// A customer picker row: identified by the customer id, displayed by name.
/// The empty id is the Walk-in option (the web select's `<option value="">`).
/// `pub(crate)`: the training screen reuses the picker for its
/// attach-customer lesson.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CustomerItem {
    pub(crate) id: String,
    pub(crate) label: String,
}

impl SearchableListItem for CustomerItem {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone().into()
    }

    fn value(&self) -> &String {
        &self.id
    }
}

pub(crate) fn customer_items(customers: &[Customer]) -> Vec<CustomerItem> {
    let mut items = vec![CustomerItem {
        id: String::new(),
        label: "Walk-in".to_string(),
    }];
    items.extend(customers.iter().map(|customer| CustomerItem {
        id: customer.id.clone(),
        // A customer added from a typed number has no name yet.
        label: if customer.name.trim().is_empty() { customer.phone.clone() } else { customer.name.clone() },
    }));
    items
}

// ── Small helpers ─────────────────────────────────────────────────────────

/// A product carries knowledge when any of the read-only peek fields is
/// filled: the web `hasKnowledge` contract decides the corner affordance.
pub(crate) fn has_knowledge(product: &Product) -> bool {
    !product.description.is_empty()
        || !product.prep_notes.is_empty()
        || !product.allergens.is_empty()
}

/// The web `.kpopover` body: name, description, then Prep and Allergens
/// under uppercase kickers. Read-only; 290px like the web width. Takes
/// colors by value so the returned element borrows nothing (popover
/// content closures must return owned elements).
fn knowledge_peek_card(product: Product, muted: Hsla, foreground: Hsla) -> impl IntoElement {
    v_flex()
        .w(px(290.))
        .gap_1()
        .child(
            div()
                .text_size(crate::theme::metrics::FONT_EMPHASIS)
                .font_weight(FontWeight::MEDIUM)
                .child(SharedString::from(product.name.clone())),
        )
        .when(!product.description.is_empty(), |card| {
            card.child(
                div()
                    .text_size(crate::theme::metrics::FONT_BODY)
                    .text_color(foreground)
                    .child(SharedString::from(product.description.clone())),
            )
        })
        .when(!product.prep_notes.is_empty(), |card| {
            card.child(
                v_flex()
                    .gap_0p5()
                    .mt_1()
                    .child(
                        div()
                            .text_size(crate::theme::metrics::FONT_CAPTION)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(muted)
                            .child("PREP"),
                    )
                    .child(
                        div()
                            .text_size(crate::theme::metrics::FONT_BODY)
                            .child(SharedString::from(product.prep_notes.clone())),
                    ),
            )
        })
        .when(!product.allergens.is_empty(), |card| {
            card.child(
                v_flex()
                    .gap_0p5()
                    .mt_1()
                    .child(
                        div()
                            .text_size(crate::theme::metrics::FONT_CAPTION)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(muted)
                            .child("ALLERGENS"),
                    )
                    .child(
                        div()
                            .text_size(crate::theme::metrics::FONT_BODY)
                            .child(SharedString::from(product.allergens.clone())),
                    ),
            )
        })
}

pub(crate) fn fmt_money(minor: i64, currency: &Currency) -> String {
    money::format(&money::money(minor, currency.code), Some(currency)).unwrap_or_default()
}

/// A quantity as a cashier reads it: whole units bare, weighed goods to at
/// most three decimals ("2", "0.25").
fn qty_label(qty: f64) -> String {
    if qty.fract() == 0.0 {
        format!("{}", qty as i64)
    } else {
        format!("{qty:.3}").trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// A money amount without its currency symbol ("30.000"), for columns
/// where the symbol would repeat on every row.
fn fmt_number(minor: i64, currency: &Currency) -> String {
    let full = fmt_money(minor, currency);
    match full.find(|c: char| c.is_ascii_digit()) {
        Some(at) if !full[..at].contains('-') => full[at..].to_string(),
        _ => full,
    }
}

/// Basis points as a percentage label: 1000 -> "10", 1250 -> "12.5".
fn percent_label(bps: i64) -> String {
    if bps % 100 == 0 {
        (bps / 100).to_string()
    } else {
        format!("{:.2}", bps as f64 / 100.0).trim_end_matches('0').to_string()
    }
}

fn plural(count: i64, noun: &str) -> String {
    if count == 1 { format!("1 {noun}") } else { format!("{count} {noun}s") }
}

fn plural_units(units: f64) -> String {
    if units == 1.0 { "1 item".to_string() } else { format!("{} items", qty_label(units)) }
}

/// Local wall-clock `HH:MM` of an ISO timestamp (the shift's opening).
fn clock_of(iso: &str) -> String {
    use kaseer_core::chrono::{DateTime, Local};
    DateTime::parse_from_rfc3339(iso)
        .map(|at| at.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_else(|_| iso.chars().skip(11).take(5).collect())
}

fn days_ago(days: i64) -> String {
    match days {
        0 => "today".to_string(),
        1 => "yesterday".to_string(),
        days => format!("{days} days ago"),
    }
}

/// A key cap plus what the key does, for the empty cart's two hints.
fn key_hint(key: &'static str, action: &'static str, _border: Hsla) -> impl IntoElement {
    h_flex()
        .gap(px(6.))
        .items_center()
        .child(super::key_cap(key))
        .child(action)
}

/// Up to two initials from a product name ("Es Kopi Coklat" -> "EK").
fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

fn digits_of(value: &str) -> String {
    value.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn cost_map(products: &[Product]) -> HashMap<String, i64> {
    products.iter().filter(|p| p.cost_minor > 0).map(|p| (p.id.clone(), p.cost_minor)).collect()
}

fn stock_map(repo: &Repo, products: &[Product]) -> HashMap<String, i64> {
    products
        .iter()
        .filter(|product| product.track_stock)
        .filter_map(|product| {
            repo.stock_on_hand(&product.id)
                .ok()
                .map(|on_hand| (product.id.clone(), on_hand))
        })
        .collect()
}

pub(crate) fn unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos()
}

/// Wall-clock UTC in the same ISO-8601 shape the web app posts
/// (`2026-09-27T08:15:42.123Z`). The app crate owns no chrono dependency,
/// so the civil-date conversion lives here (Howard Hinnant's algorithm).
pub(crate) fn now_iso() -> String {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the unix epoch");
    // The "." matters: without it the stamp ("...:42123Z") is not ISO, and
    // every reader (parse_ms, the cloud) fails on it.
    format!(
        "{}.{:03}Z",
        civil_timestamp(since_epoch.as_secs() as i64),
        since_epoch.subsec_millis()
    )
}

fn civil_timestamp(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let (hour, minute, second) = (
        secs_of_day / 3_600,
        (secs_of_day % 3_600) / 60,
        secs_of_day % 60,
    );
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}")
}

#[cfg(test)]
mod tests {
    use super::{
        civil_timestamp, days_ago, now_iso, percent_label, plural_units, qty_label, quick_cash,
    };

    #[test]
    fn quick_cash_offers_the_next_round_amounts() {
        assert_eq!(quick_cash(15_000), vec![16_000, 20_000, 50_000]);
        assert_eq!(quick_cash(48_000), vec![50_000, 60_000, 100_000]);
        assert_eq!(quick_cash(60_000), vec![100_000, 150_000, 200_000]);
        assert!(quick_cash(0).is_empty());
    }

    #[test]
    fn percent_labels_drop_trailing_zeros() {
        assert_eq!(percent_label(1000), "10");
        assert_eq!(percent_label(1250), "12.5");
    }

    #[test]
    fn now_iso_is_a_parseable_iso_stamp() {
        let stamp = now_iso();
        assert_eq!(stamp.len(), "2026-09-27T08:15:42.123Z".len(), "{stamp}");
        assert_eq!(&stamp[19..20], ".", "{stamp}");
        assert!(kaseer_core::reports::parse_ms(&stamp).is_some(), "{stamp}");
    }

    #[test]
    fn quantities_and_recency_read_like_a_cashier_says_them() {
        assert_eq!(qty_label(2.0), "2");
        assert_eq!(qty_label(0.25), "0.25");
        assert_eq!(qty_label(1.5), "1.5");
        assert_eq!(plural_units(1.0), "1 item");
        assert_eq!(plural_units(0.0), "0 items");
        assert_eq!(plural_units(4.0), "4 items");
        assert_eq!(days_ago(0), "today");
        assert_eq!(days_ago(1), "yesterday");
        assert_eq!(days_ago(47), "47 days ago");
    }

    #[test]
    fn civil_timestamp_matches_known_instants() {
        assert_eq!(civil_timestamp(0), "1970-01-01T00:00:00");
        assert_eq!(civil_timestamp(-1), "1969-12-31T23:59:59");
        // Leap day and a recent known instant.
        assert_eq!(civil_timestamp(951_782_400), "2000-02-29T00:00:00");
        assert_eq!(civil_timestamp(1_700_000_000), "2023-11-14T22:13:20");
    }
}
