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

use std::collections::{HashMap, HashSet};
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
    ActiveTheme as _, Disableable as _, Icon, IndexPath, Sizable as _, WindowExt,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::prelude::StyledImage as _;
use gpui_kit::{
    Anchor, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, img, px,
};
use kaseer_core::bundles::{BundlePair, co_purchase_pairs, partner_suggestions};
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
use kaseer_core::payments::{self, PaymentState, Tender, TenderMethod};
use kaseer_core::receipt::{Sale, SaleLine, SaleTender, StoreInfo, render_receipt_text};
use kaseer_core::repos::{Customer, Product, Repo, Settings};
use kaseer_core::shift::Shift;
use kaseer_core::tax::TaxRate;
use kaseer_core::winback::{Recognition, recognition};

/// Pairs need this many shared sales before the register suggests them:
/// one shared basket is coincidence (same floor as kaseer-web).
const MIN_PAIR_SUPPORT: i64 = 2;
/// "Often bought with" shows at most this many partners.
const MAX_SUGGESTIONS: usize = 2;

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
    /// Co-purchase pairs over the whole sale history, strongest first.
    /// Computed off the UI thread at startup and after every settled sale;
    /// empty until then, which hides the suggestion card.
    bundle_pairs: Vec<BundlePair>,
    /// The attached customer's visit summary, keyed by customer id so it
    /// is queried once per attach, not per frame.
    recognition: Option<(String, Recognition)>,
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
            bundle_pairs: Vec::new(),
            recognition: None,
            _subscriptions: Vec::new(),
        };

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
                        cx.notify();
                    }
                }),
            );

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
        let path = self.repo.db_path().to_string();
        cx.spawn(async move |this, cx| {
            let pairs = cx
                .background_executor()
                .spawn(async move {
                    let repo = open_repo(&path).ok()?;
                    let sales = repo
                        .sales_between("0000-01-01T00:00:00.000Z", "9999-12-31T23:59:59.999Z")
                        .ok()?;
                    Some(co_purchase_pairs(&sales, usize::MAX))
                })
                .await;
            if let Some(pairs) = pairs {
                let _ = this.update(cx, |this, cx| {
                    this.bundle_pairs = pairs;
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

    /// Up to two products people bought with what is in the cart, from
    /// real sale history only. Empty means the card stays hidden.
    fn suggestions(&self) -> Vec<(Product, String, i64)> {
        let basket: HashSet<&str> = self
            .cart
            .lines
            .iter()
            .map(|line| line.product_id.as_str())
            .collect();
        if basket.is_empty() {
            return vec![];
        }
        partner_suggestions(&self.bundle_pairs, &basket, MIN_PAIR_SUPPORT, MAX_SUGGESTIONS, |id| {
            self.products.iter().any(|product| product.id == id)
        })
        .into_iter()
        .filter_map(|suggestion| {
            let product = self.products.iter().find(|p| p.id == suggestion.product_id)?;
            // The anchor's current catalog name beats the first-seen name
            // from history when the product was renamed since.
            let anchor = self
                .products
                .iter()
                .find(|p| p.id == suggestion.anchor_id)
                .map(|p| p.name.clone())
                .unwrap_or(suggestion.anchor_name);
            Some((product.clone(), anchor, suggestion.support))
        })
        .collect()
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
    fn consume_pending_menu_revision(&mut self) {
        let Some(rev) = self.pending_menu_rev.take() else {
            return;
        };
        self.menu_rev_seen = rev;
        if let Ok(products) = self.repo.products(false, None) {
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
        match cart::add_to_cart(&self.cart, &product.addable(), 1.0) {
            Ok(next) => {
                self.cart = next;
                cx.notify();
            }
            Err(error) => window.push_notification(Notification::error(error.to_string()), cx),
        }
        self.query
            .update(cx, |input, cx| input.set_value("", window, cx));
    }

    fn set_line_qty(&mut self, line_id: &str, qty: f64, cx: &mut Context<Self>) {
        // At the 9_999 cap an extra + is ignored rather than crashing the
        // register (the web version would panic inside its state updater).
        if let Ok(next) = cart::set_line_qty(&self.cart, line_id, qty) {
            self.cart = next;
            cx.notify();
        }
    }

    fn remove_line(&mut self, line_id: &str, cx: &mut Context<Self>) {
        self.cart = cart::remove_line(&self.cart, line_id);
        cx.notify();
    }

    fn apply_order_percent(&mut self, cx: &mut Context<Self>) {
        let raw = self.percent.read(cx).value().trim().to_string();
        let bps = raw
            .parse::<f64>()
            .map(|value| (value * 100.0).round() as i64)
            .unwrap_or(0);
        self.cart =
            cart::set_order_discount(&self.cart, (bps > 0).then_some(Discount::Percent { bps }));
        cx.notify();
    }

    fn clear_order_percent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cart = cart::set_order_discount(&self.cart, None);
        self.percent
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    // ── Pay flow ──────────────────────────────────────────────────────────

    pub fn open_pay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let total_minor = self.totals().total_minor;
        let customer = self.attached_customer().map(|c| CustomerSnapshot {
            id: c.id,
            name: c.name,
            points: c.points,
        });
        let loyalty = self.settings.loyalty.clone();
        let currency = self.currency;
        self.pay_dialog.update(cx, |dialog, cx| {
            dialog.reset(total_minor, customer, loyalty, currency, window, cx)
        });
        let pay = self.pay_dialog.clone();
        let surface = cx.theme().popover;
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Payment")
                .w(px(640.))
                .bg(surface)
                .child(pay.clone())
        });
    }

    fn on_pay_event(&mut self, event: &PayEvent, window: &mut Window, cx: &mut Context<Self>) {
        let PayEvent::Paid {
            tenders,
            redeem_points,
        } = event;
        match self.checkout(tenders, *redeem_points, cx) {
            Ok(sale) => {
                window.close_dialog(cx);
                self.refresh_catalog(window, cx);
                // The new sale is history now: pairs and visit counts move.
                self.refresh_bundles(cx);
                self.recognition = None;
                self.open_receipt(sale, window, cx);
            }
            Err(message) => window.push_notification(Notification::error(message), cx),
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
        self.receipt_dialog
            .update(cx, |dialog, _| dialog.show(sale, store));
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

    fn render_catalog(&self, cx: &Context<Self>, products: &[Product]) -> impl IntoElement {
        v_flex()
            .flex_grow(1.6)
            .min_w_0()
            .gap(px(10.))
            // The input's editor renders size_full: placed bare in a flex
            // column it would take the whole column height and starve the
            // grid. The fixed-height wrapper pins it to one line.
            .child(
                div()
                    .h(px(32.))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .child(Input::new(&self.query).id("search")),
            )
            .child(self.render_category_chips(cx))
            .child(self.render_product_grid(cx, products))
    }

    /// One filter chip. Category chips lead with their hue dot so the chip
    /// row doubles as the colour key for the grid. The active chip is the
    /// one accented element in the row (soft fill, accent outline).
    fn render_category_chip(
        &self,
        cx: &Context<Self>,
        id: SharedString,
        label: SharedString,
        hue: Option<Hsla>,
        active: bool,
        category: Option<String>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .id(id)
            .cursor_pointer()
            .flex_none()
            .h(px(26.))
            .px(px(10.))
            .gap(px(6.))
            .items_center()
            .rounded(px(999.))
            .border_1()
            .text_size(crate::theme::metrics::FONT_BODY)
            .map(|chip| {
                if active {
                    chip.bg(crate::theme::accent_soft())
                        .border_color(theme.accent)
                        .text_color(theme.foreground)
                        .font_weight(FontWeight::MEDIUM)
                } else {
                    chip.border_color(crate::theme::border_strong())
                        .text_color(theme.muted_foreground)
                        .hover(|style| style.border_color(theme.muted_foreground))
                }
            })
            .when_some(hue, |chip, hue| {
                chip.child(div().size(px(6.)).flex_none().rounded(px(999.)).bg(hue))
            })
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.category = category.clone();
                cx.notify();
            }))
    }

    fn render_category_chips(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut row = h_flex()
            .gap(px(6.))
            .flex_none()
            .flex_shrink_0()
            .h(px(28.))
            .overflow_x_scrollbar();
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

    fn render_product_grid(&self, cx: &Context<Self>, products: &[Product]) -> impl IntoElement {
        if products.is_empty() {
            return div()
                .flex_grow(1.)
                .min_h_0()
                .pt_8()
                .text_center()
                .text_color(cx.theme().muted_foreground)
                .child("No products match.")
                .into_any_element();
        }
        // The web grid is `repeat(auto-fill, minmax(170px, 1fr))`; gpui
        // tracks are fixed, so 4 columns at the 1200px window floor keeps
        // every tile at or above the 170px minimum.
        // Rows pack to the top at their content height: stretched rows
        // left uneven gaps once tiles lost the barcode line, and a single
        // search hit would balloon to the full grid height.
        let mut grid = div()
            .grid()
            .grid_cols(4)
            .content_start()
            .gap(px(8.))
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

    fn render_product_card(
        &self,
        product: &Product,
        banded: bool,
        in_cart: Option<f64>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let known = has_knowledge(product);
        let hue = crate::theme::category_hue(&self.categories, &product.category);
        let low_note = self.low_stock_note(product);
        // Photo band (the web `.band`): pulled from the companion into the
        // ledger's images/ dir. An empty path or a not-yet-downloaded file
        // keeps the text-only card until a later pull lands the file.
        let photo = self.photo_path(product);
        let body = v_flex()
            .w_full()
            .flex_grow(1.)
            .gap(px(8.))
            .pt(px(10.))
            .pb(px(10.))
            .pl(px(10.))
            .pr(px(10.))
            // Corner overlays (badge, knowledge button) never sit on the
            // name: reserve their width on the first lines.
            .when(known || in_cart.is_some(), |body| body.pr(px(30.)))
            .when(known && in_cart.is_some(), |body| body.pr(px(56.)))
            .child(
                div()
                    .text_size(crate::theme::metrics::FONT_EMPHASIS)
                    .line_height(px(18.))
                    .font_weight(FontWeight::MEDIUM)
                    // The name is the tile's hero: two lines before the
                    // ellipsis, never shredded at narrow widths.
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
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::theme::metrics::FONT_FOOTNOTE)
                            .text_color(theme.danger)
                            .when_some(low_note, |note, text| note.child(SharedString::from(text))),
                    )
                    .child(
                        super::money_text(fmt_money(product.price_minor, self.currency))
                            .flex_none()
                            .text_size(crate::theme::metrics::FONT_EMPHASIS)
                            .text_color(theme.muted_foreground),
                    ),
            );
        // Category edge: the shell is filled with the hue and the surface
        // sits 4px in from its left, so the hue reads as a band inside the
        // rounded border. (GPUI clips overflow to a rectangle, so a 4px
        // child pinned to the left would poke out square at the corners.)
        let inner_radius = crate::theme::CARD_RADIUS - px(1.);
        let surface = v_flex()
            .flex_1()
            .min_w_0()
            .ml(px(3.))
            .bg(theme.secondary)
            .rounded_tr(inner_radius)
            .rounded_br(inner_radius)
            .map(|surface| match photo {
                Some(path) => surface.child(
                    img(path)
                        .id(SharedString::from(format!("product-img-{}", product.id)))
                        .w_full()
                        .h(px(96.))
                        .flex_none()
                        .rounded_tr(inner_radius)
                        .object_fit(ObjectFit::Cover),
                ),
                // No photo yet: the product's initials on the raised
                // surface, an honest placeholder rather than a stock image.
                None if banded => surface.child(
                    div()
                        .w_full()
                        .h(px(96.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_tr(inner_radius)
                        .bg(theme.muted)
                        .text_size(crate::theme::metrics::FONT_DISPLAY)
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
            .min_h(px(88.))
            .overflow_hidden()
            .rounded(crate::theme::CARD_RADIUS)
            .bg(hue)
            .border_1()
            .map(|card| {
                if in_cart.is_some() {
                    card.border_color(theme.accent)
                } else {
                    card.border_color(crate::theme::tile_border())
                        .hover(|style| style.border_color(crate::theme::accent_dim()))
                }
            })
            .active(|style| style.opacity(0.85))
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
                .top(px(8.))
                .right(px(8.))
                .h(px(20.))
                .min_w(px(20.))
                .px(px(5.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(999.))
                // Atur's primary: the light accent with black digits.
                .bg(theme.accent)
                .text_color(theme.accent_foreground)
                .font_family(crate::theme::FONT_MONEY)
                .font_features(crate::theme::tabular())
                .text_size(crate::theme::metrics::FONT_FOOTNOTE)
                .font_weight(FontWeight::SEMIBOLD)
                .child(SharedString::from(qty_label(qty)))
        });
        let knowledge = known.then(|| {
            // The web `.kinfo`: a small corner button floating over the
            // card's top-right, opening a click-away knowledge popover. It
            // steps left of the quantity badge when both are present.
            div()
                .absolute()
                .top(px(6.))
                .right(px(if in_cart.is_some() { 32. } else { 6. }))
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
                    .small()
                    .icon(Icon::new(IconName::Info).with_size(px(13.))),
            )
            .content(move |_, _, _| knowledge_peek_card(product.clone(), muted, foreground))
    }

    fn render_cart_panel(
        &self,
        cx: &Context<Self>,
        totals: &CartTotals,
        customer: Option<Customer>,
        amount_size: Pixels,
    ) -> impl IntoElement {
        let suggestions = self.suggestions();
        v_flex()
            .flex_grow(1.)
            // Let the panel shrink with the window row instead of sizing to
            // its content: unconstrained, the lines list keeps its content
            // height and its scroll hitbox spills over totals/pay.
            .min_h_0()
            .overflow_hidden()
            .max_w(px(400.))
            .min_w_0()
            .gap(px(10.))
            .p(px(12.))
            .rounded(crate::theme::CARD_RADIUS)
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(self.render_cart_header(cx))
            .when_some(customer, |panel, customer| {
                panel.child(self.render_recognition(&customer, cx))
            })
            .child(self.render_cart_lines(cx))
            .when(!suggestions.is_empty(), |panel| {
                panel.child(self.render_suggestions(suggestions, cx))
            })
            .child(self.render_total_block(cx, totals, amount_size))
    }

    fn render_cart_header(&self, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .justify_between()
            .items_center()
            .gap_2()
            .flex_none()
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(crate::theme::metrics::FONT_FOOTNOTE)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(cx.theme().muted_foreground)
                    .child("CURRENT SALE"),
            )
            // Walk-in stays quiet: the attach control is the whole header.
            .child(
                div()
                    .flex_none()
                    .w(px(170.))
                    .child(Select::new(&self.customer_select)),
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
        if self.cart.lines.is_empty() {
            // One line and the two keys that start and finish a sale. The
            // bottom of the column carries the weight, not this hint.
            return v_flex()
                .flex_grow(1.)
                .min_h_0()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .child(
                    div()
                        .text_size(crate::theme::metrics::FONT_EMPHASIS)
                        .text_color(cx.theme().muted_foreground)
                        .child("Scan a barcode or tap a product to start."),
                )
                .child(
                    h_flex()
                        .gap(px(16.))
                        .text_size(crate::theme::metrics::FONT_FOOTNOTE)
                        .text_color(crate::theme::text_dim())
                        .child(key_hint("Enter", "adds a scan", cx.theme().border))
                        .child(key_hint("F2", "takes payment", cx.theme().border)),
                )
                .into_any_element();
        }
        // The discount row rides at the end of the lines it discounts; the
        // free space falls below it, so suggestions and the total anchor
        // the bottom of the column instead of a stranded input.
        let mut list = v_flex()
            .flex_grow(1.)
            .min_h_0()
            .gap(px(2.))
            .overflow_y_scrollbar();
        for line in &self.cart.lines {
            list = list.child(self.render_cart_line(line, cx));
        }
        list.child(div().pt(px(8.)).child(self.render_discount_row(cx)))
            .into_any_element()
    }

    fn render_cart_line(&self, line: &cart::CartLine, cx: &Context<Self>) -> impl IntoElement {
        let gross_minor = (line.unit_minor as f64 * line.qty).round() as i64;
        let hue = self
            .products
            .iter()
            .find(|p| p.id == line.product_id)
            .map(|p| crate::theme::category_hue(&self.categories, &p.category));
        h_flex()
            .justify_between()
            .gap_2()
            .py(px(6.))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .min_w_0()
                    .gap(px(10.))
                    .items_start()
                    // The same hue dot as the tile edge and chip: the cart
                    // reads back to the grid region an item came from.
                    .child(
                        div()
                            .mt(px(6.))
                            .size(px(6.))
                            .flex_none()
                            .rounded(px(999.))
                            .when_some(hue, |dot, hue| dot.bg(hue)),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(crate::theme::metrics::FONT_EMPHASIS)
                                    .font_weight(FontWeight::MEDIUM)
                                    .truncate()
                                    .child(SharedString::from(line.name.clone())),
                            )
                            .child(
                                super::money_text(format!(
                                    "{} × {}",
                                    fmt_money(line.unit_minor, self.currency),
                                    qty_label(line.qty)
                                ))
                                .text_size(crate::theme::metrics::FONT_FOOTNOTE)
                                .text_color(cx.theme().muted_foreground),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new(SharedString::from(format!("qty-dec-{}", line.id)))
                            .ghost()
                            .small()
                            .label("−")
                            .on_click(cx.listener({
                                let line_id = line.id.clone();
                                let qty = line.qty - 1.0;
                                move |this, _, _, cx| this.set_line_qty(&line_id, qty, cx)
                            })),
                    )
                    // 96px holds Rp 999.999 in Plex Mono; nowrap keeps it
                    // on one line.
                    .child(
                        super::money_text(fmt_money(gross_minor, self.currency))
                            .w(px(88.))
                            .text_right()
                            .text_size(crate::theme::metrics::FONT_EMPHASIS),
                    )
                    .child(
                        Button::new(SharedString::from(format!("qty-inc-{}", line.id)))
                            .ghost()
                            .small()
                            .label("+")
                            .on_click(cx.listener({
                                let line_id = line.id.clone();
                                let qty = line.qty + 1.0;
                                move |this, _, _, cx| this.set_line_qty(&line_id, qty, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("line-remove-{}", line.id)))
                            .ghost()
                            .small()
                            .label("×")
                            .on_click(cx.listener({
                                let line_id = line.id.clone();
                                move |this, _, _, cx| this.remove_line(&line_id, cx)
                            })),
                    ),
            )
    }

    fn render_discount_row(&self, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_2()
            .items_center()
            .flex_none()
            .child(
                div()
                    .w(px(120.))
                    .child(Input::new(&self.percent).id("disc-percent")),
            )
            .child(
                Button::new("discount-apply")
                    .label("Apply %")
                    .on_click(cx.listener(|this, _, _, cx| this.apply_order_percent(cx))),
            )
            .when(self.cart.order_discount.is_some(), |row| {
                row.child(
                    Button::new("discount-clear")
                        .small()
                        .ghost()
                        .label("Clear")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.clear_order_percent(window, cx)),
                        ),
                )
            })
    }

    /// "Often bought with": partners from real co-purchase history, with
    /// the evidence first so the cashier can say it out loud ("27 sales
    /// with Kopi Susu"). The Add button puts the product in the cart.
    fn render_suggestions(
        &self,
        suggestions: Vec<(Product, String, i64)>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let mut card = v_flex()
            .flex_none()
            .gap(px(8.))
            .p(px(10.))
            .rounded(crate::theme::metrics::RADIUS_LG)
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap(px(8.))
                    .items_center()
                    .text_size(crate::theme::metrics::FONT_FOOTNOTE)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(IconName::Link2).with_size(px(12.)).text_color(theme.muted_foreground))
                    .child("OFTEN BOUGHT WITH"),
            );
        for (product, anchor, support) in suggestions {
            let product_id = product.id.clone();
            card = card.child(
                h_flex()
                    .gap(px(10.))
                    .items_center()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(1.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(crate::theme::metrics::FONT_FOOTNOTE)
                                    .text_color(crate::theme::text_dim())
                                    .child(SharedString::from(format!("{support} sales with {anchor}"))),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(crate::theme::metrics::FONT_EMPHASIS)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(SharedString::from(product.name.clone())),
                            ),
                    )
                    .child(
                        super::money_text(fmt_money(product.price_minor, self.currency))
                            .flex_none()
                            .text_size(crate::theme::metrics::FONT_BODY)
                            .text_color(theme.muted_foreground)
                            .text_right(),
                    )
                    .child(
                        Button::new(SharedString::from(format!("suggest-add-{}", product.id)))
                            .secondary()
                            .small()
                            .icon(Icon::new(IconName::Plus).with_size(px(13.)))
                            .label("Add")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(product) =
                                    this.products.iter().find(|p| p.id == product_id).cloned()
                                {
                                    this.add_product(&product, window, cx);
                                }
                            })),
                    ),
            );
        }
        card
    }

    /// The total block: the sale's money and its one action. A recessed
    /// well on the canvas colour inside the cart panel, the same layout
    /// whether or not the sale is live so nothing jumps when the first item
    /// lands. Only the Pay action carries the accent (Atur's primary
    /// control: light gray, black type).
    fn render_total_block(
        &self,
        cx: &Context<Self>,
        totals: &CartTotals,
        amount_size: Pixels,
    ) -> impl IntoElement {
        use crate::theme::metrics;
        let theme = cx.theme();
        let has_items = !self.cart.lines.is_empty();
        let has_shift = self.active_shift.is_some();
        let live = has_items && has_shift;
        let (strong, soft) = (theme.foreground, theme.muted_foreground);
        let units: f64 = self.cart.lines.iter().map(|line| line.qty).sum();
        let row = |label: &'static str, value: String| {
            h_flex()
                .justify_between()
                .text_size(metrics::FONT_BODY)
                .text_color(soft)
                .child(label)
                .child(super::money_text(value).text_right())
        };
        let note = match (has_shift, has_items) {
            (false, _) => Some("No open shift. Open one to take payment."),
            (true, false) => Some("Add an item to take payment."),
            (true, true) => None,
        };
        v_flex()
            .id("total-block")
            .flex_none()
            .gap(px(6.))
            .p(px(12.))
            .rounded(metrics::RADIUS_LG)
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .child(
                div()
                    .font_family(crate::theme::FONT_MONO)
                    .text_size(metrics::FONT_CAPTION)
                    .text_color(crate::theme::text_dim())
                    .child(SharedString::from(plural_units(units).to_uppercase())),
            )
            .child(row("Subtotal", fmt_money(totals.gross_minor, self.currency)))
            .when(totals.discount_minor > 0, |block| {
                block.child(row(
                    "Discount",
                    format!("-{}", fmt_money(totals.discount_minor, self.currency)),
                ))
            })
            .child(row("Tax", fmt_money(totals.tax_minor, self.currency)))
            .child(div().h(px(1.)).my(px(4.)).bg(theme.border))
            .child(
                h_flex()
                    .justify_between()
                    .items_end()
                    .gap_2()
                    .child(
                        div()
                            .pb(px(6.))
                            .text_size(metrics::FONT_EMPHASIS)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(soft)
                            .child("Total"),
                    )
                    .child(
                        super::money_text(fmt_money(totals.total_minor, self.currency))
                            .text_right()
                            .text_size(amount_size)
                            .line_height(amount_size * 1.15)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if has_items { strong } else { soft }),
                    ),
            )
            .when_some(note, |block, note| {
                block.child(
                    div()
                        .text_size(metrics::FONT_FOOTNOTE)
                        .text_color(soft)
                        .child(note),
                )
            })
            .child(div().pt(px(6.)).child(self.render_total_action(cx, live, has_shift)))
    }

    /// The block's one button. Pay (live) and Open shift (no shift) are the
    /// accent-filled primary; with a shift but an empty cart it is a
    /// visibly locked Pay, since there is nothing to charge yet.
    fn render_total_action(&self, cx: &Context<Self>, live: bool, has_shift: bool) -> impl IntoElement {
        use crate::theme::metrics;
        let theme = cx.theme();
        let (label, key) = if has_shift { ("Pay", "F2") } else { ("Open shift", "F7") };
        let enabled = live || !has_shift;
        let (fg, key_border) = if enabled {
            (theme.accent_foreground, theme.accent_foreground.opacity(0.25))
        } else {
            (crate::theme::text_dim(), theme.border)
        };
        let (accent, hover, active) = (theme.primary, theme.primary_hover, theme.primary_active);
        h_flex()
            .id(if has_shift { "pay" } else { "open-shift" })
            .w_full()
            .h(px(44.))
            .gap(px(10.))
            .items_center()
            .justify_center()
            .rounded(metrics::RADIUS_MD)
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
                div()
                    .text_size(metrics::FONT_HEADLINE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(label),
            )
            .child(
                div()
                    .px(px(5.))
                    .rounded(metrics::RADIUS_SM)
                    .border_1()
                    .border_color(key_border)
                    .font_family(crate::theme::FONT_MONO)
                    .text_size(metrics::FONT_CAPTION)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(key),
            )
    }
}

impl Render for RegisterScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A menu pull landed while we were idle: reload the snapshot here,
        // in this view's own render pass (see spawn_menu_watch).
        self.consume_pending_menu_revision();
        // Customers' "Sell to" hands its pick over through the
        // SelectedCustomer global; consume it before reading the cart.
        self.consume_selected_customer(window, cx);
        self.sync_recognition();
        let totals = self.totals();
        let products = self.filtered_products(cx);
        let customer = self.attached_customer();
        // 42px total, 36px when the cart column would fall under 340px
        // (the web breakpoint). The column is 1/2.6 of the screen width
        // after the rail and gutters, capped at 420px.
        let column = ((f32::from(window.viewport_size().width) - 68. - 42.) / 2.6).min(400.);
        let amount_size = if column < 340. { px(28.) } else { px(32.) };

        h_flex()
            .size_full()
            .items_stretch()
            .gap(px(12.))
            .p(px(12.))
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_catalog(cx, &products))
            .child(self.render_cart_panel(cx, &totals, customer, amount_size))
    }
}

// ── Pay dialog ────────────────────────────────────────────────────────────

/// What the pay dialog reports back to the register (and the training
/// sandbox, which reuses this dialog verbatim).
pub(crate) enum PayEvent {
    Paid {
        tenders: Vec<Tender>,
        redeem_points: i64,
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

/// Payment modal: tender composition against the running total. Cash may
/// overpay (that is what change is); the other methods cannot; loyalty
/// points ride in as their own tender. `pub(crate)`: the training screen
/// reuses this exact dialog for its payment lesson.
pub(crate) struct PayDialog {
    total_minor: i64,
    customer: Option<CustomerSnapshot>,
    loyalty: LoyaltyConfig,
    currency: &'static Currency,
    tenders: Vec<Tender>,
    entry: Entity<InputState>,
    redeem: Entity<InputState>,
}

impl PayDialog {
    pub(crate) fn new(
        currency: &'static Currency,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let entry = cx.new(|cx| {
            InputState::new(window, cx).placeholder(format!("Amount ({})", currency.code))
        });
        let redeem = cx.new(|cx| InputState::new(window, cx).placeholder("Points"));
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
            entry,
            redeem,
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
        self.entry
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.redeem
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    fn entry_minor(&self, cx: &Context<Self>) -> i64 {
        digits_of(&self.entry.read(cx).value()).parse().unwrap_or(0)
    }

    fn clear_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.entry
            .update(cx, |input, cx| input.set_value("", window, cx));
    }

    fn add_method_tender(
        &mut self,
        method: TenderMethod,
        remaining: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = self.entry_minor(cx);
        let wanted = if entry > 0 { entry } else { remaining };
        let applied = wanted.min(remaining);
        if applied <= 0 {
            return;
        }
        // Cash records what was handed over (its unclamped entry); the rest
        // cannot exceed the balance.
        let given = (method == TenderMethod::Cash).then_some(wanted);
        self.tenders
            .push(payments::new_tender(method, applied, given));
        self.clear_entry(window, cx);
        cx.notify();
    }

    fn add_quick_cash(&mut self, note: i64, window: &mut Window, cx: &mut Context<Self>) {
        let remaining = payments::evaluate_payments(self.total_minor, &self.tenders)
            .remaining_minor
            .max(0);
        let applied = note.min(remaining);
        if applied <= 0 {
            return;
        }
        self.tenders.push(payments::new_tender(
            TenderMethod::Cash,
            applied,
            Some(note),
        ));
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

    fn clamped_redeem_points(&self, max_redeem: i64, cx: &Context<Self>) -> i64 {
        digits_of(&self.redeem.read(cx).value())
            .parse::<i64>()
            .unwrap_or(0)
            .clamp(0, max_redeem)
    }

    fn render_total_band(&self, payment: &PaymentState, cx: &Context<Self>) -> impl IntoElement {
        let (label, value, color) = if payment.complete {
            ("CHANGE", payment.change_minor, cx.theme().success)
        } else {
            (
                "REMAINING",
                payment.remaining_minor.max(0),
                cx.theme().accent,
            )
        };
        h_flex()
            .justify_between()
            .rounded(cx.theme().radius)
            .bg(cx.theme().background)
            .px_3()
            .py_2()
            // The band used to label TOTAL without printing it.
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("TOTAL"),
                    )
                    .child(
                        super::money_text(fmt_money(self.total_minor, self.currency))
                            .text_xl()
                            .font_semibold(),
                    ),
            )
            .child(
                v_flex()
                    .gap_0p5()
                    .items_end()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_xl()
                            .font_semibold()
                            .font_family(crate::theme::FONT_MONEY)
                            .font_features(crate::theme::tabular())
                            .text_color(color)
                            .child(SharedString::from(fmt_money(value, self.currency))),
                    ),
            )
    }

    fn render_methods(&self, remaining: i64, cx: &Context<Self>) -> impl IntoElement {
        let methods = [
            (TenderMethod::Cash, "Cash"),
            (TenderMethod::Card, "Card"),
            (TenderMethod::Qris, "QRIS"),
            (TenderMethod::Transfer, "Transfer"),
        ];
        let mut row = h_flex().gap_2().flex_wrap();
        for (method, label) in methods {
            row = row.child(
                Button::new(SharedString::from(format!("method-{}", method.as_str())))
                    .secondary()
                    .label(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.add_method_tender(method, remaining, window, cx)
                    })),
            );
        }
        row
    }

    fn render_amount_entry(&self, remaining: i64, cx: &Context<Self>) -> impl IntoElement {
        const QUICK_NOTES: [i64; 4] = [10_000, 20_000, 50_000, 100_000];
        let mut row = h_flex().gap_2().flex_wrap().child(
            div()
                .flex_1()
                .min_w(px(140.))
                .child(Input::new(&self.entry).id("pay-entry")),
        );
        for note in QUICK_NOTES {
            row = row.child(
                Button::new(SharedString::from(format!("cash-{note}")))
                    .secondary()
                    .label(SharedString::from(fmt_money(note, self.currency)))
                    .disabled(remaining <= 0)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.add_quick_cash(note, window, cx)
                    })),
            );
        }
        row
    }

    fn render_redeem(
        &self,
        max_redeem: i64,
        staged_value: i64,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(format!(
                        "Redeem (max {} pts = {}):",
                        max_redeem,
                        fmt_money(redeem_value_minor(max_redeem, &self.loyalty), self.currency)
                    ))),
            )
            .child(
                div()
                    .w(px(110.))
                    .child(Input::new(&self.redeem).id("pay-redeem")),
            )
            .child(
                div()
                    .text_xs()
                    .font_family(crate::theme::FONT_MONEY)
                            .font_features(crate::theme::tabular())
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(format!(
                        "= {}",
                        fmt_money(staged_value, self.currency)
                    ))),
            )
    }

    fn render_staged_tenders(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut list = v_flex().gap_1();
        for tender in &self.tenders {
            let label = match tender.given_minor {
                Some(given) => format!(
                    "{} (given {})",
                    tender.method.as_str(),
                    fmt_money(given, self.currency)
                ),
                None => tender.method.as_str().to_string(),
            };
            list = list.child(
                h_flex()
                    .justify_between()
                    .pb_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(label)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_family(crate::theme::FONT_MONEY)
                            .font_features(crate::theme::tabular())
                                    .child(SharedString::from(fmt_money(
                                        tender.applied_minor,
                                        self.currency,
                                    ))),
                            )
                            .child(
                                Button::new(SharedString::from(format!(
                                    "tender-remove-{}",
                                    tender.id
                                )))
                                .ghost()
                                .small()
                                .label("×")
                                .on_click(cx.listener({
                                    let tender_id = tender.id.clone();
                                    move |this, _, _, cx| {
                                        this.tenders.retain(|t| t.id != tender_id);
                                        cx.notify();
                                    }
                                })),
                            ),
                    ),
            );
        }
        list
    }

    fn render_num_key(&self, key: &'static str, cx: &Context<Self>) -> impl IntoElement {
        let id = if key == "⌫" { "num-back" } else { "num-key" };
        Button::new(SharedString::from(format!("{id}-{key}")))
            .secondary()
            // Web `.numgrid button`: 14px padding, 16px text: a ~48px key.
            .h(px(48.))
            .w(px(64.))
            .label(key)
            .on_click(cx.listener(move |this, _, window, cx| {
                if key == "⌫" {
                    this.backspace_entry(window, cx);
                } else {
                    this.push_entry_digits(key, window, cx);
                }
            }))
    }

    fn render_actions(
        &self,
        complete: bool,
        redeem_points: i64,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        const ROWS: [[&str; 3]; 4] = [
            ["1", "2", "3"],
            ["4", "5", "6"],
            ["7", "8", "9"],
            ["0", "00", "⌫"],
        ];
        let mut grid = v_flex().gap_2();
        for row in ROWS {
            let mut line = h_flex().gap_2();
            for key in row {
                line = line.child(self.render_num_key(key, cx));
            }
            grid = grid.child(line);
        }
        // Keypad centered on its own; the dialog's two exits share a footer
        // row, dismiss on the left and the committing Paid on the right.
        v_flex()
            .gap_3()
            .child(h_flex().justify_center().child(grid))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .pt_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("pay-cancel")
                            .ghost()
                            .label("Cancel (Esc)")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                Button::new("pay-paid")
                    .primary()
                    .large()
                    .w(px(220.))
                    .label("Paid")
                    .disabled(!complete)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.emit(PayEvent::Paid {
                            tenders: this.tenders.clone(),
                            redeem_points,
                        });
                    })),
                    ),
            )
    }
}

impl Render for PayDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let payment = payments::evaluate_payments(self.total_minor, &self.tenders);
        let remaining = payment.remaining_minor.max(0);
        let max_redeem = self
            .customer
            .as_ref()
            .map(|customer| max_redeemable_points(customer.points, self.total_minor, &self.loyalty))
            .unwrap_or(0);
        let staged_redeem = self.clamped_redeem_points(max_redeem, cx);

        v_flex()
            .gap_3()
            .child(self.render_total_band(&payment, cx))
            .child(self.render_methods(remaining, cx))
            .child(self.render_amount_entry(remaining, cx))
            .when(max_redeem > 0 && self.customer.is_some(), |dialog| {
                dialog.child(self.render_redeem(
                    max_redeem,
                    redeem_value_minor(staged_redeem, &self.loyalty),
                    cx,
                ))
            })
            .when(!self.tenders.is_empty(), |dialog| {
                dialog.child(self.render_staged_tenders(cx))
            })
            .child(self.render_actions(payment.complete, staged_redeem, cx))
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
}

impl ReceiptDialog {
    pub(crate) fn new(receipts_dir: Option<PathBuf>, currency: &'static Currency) -> Self {
        Self {
            sale: None,
            store: StoreInfo::named("Kaseer"),
            currency,
            receipts_dir,
        }
    }

    pub(crate) fn show(&mut self, sale: Sale, store: StoreInfo) {
        self.sale = Some(sale);
        self.store = store;
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
                    .justify_center()
                    .gap_2()
                    .child(
                        Button::new("receipt-export")
                            .secondary()
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
                    .child(
                        Button::new("receipt-new-sale")
                            .primary()
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
        label: customer.name.clone(),
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

fn plural(count: i64, noun: &str) -> String {
    if count == 1 { format!("1 {noun}") } else { format!("{count} {noun}s") }
}

fn plural_units(units: f64) -> String {
    if units == 1.0 { "1 item".to_string() } else { format!("{} items", qty_label(units)) }
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
    use super::{civil_timestamp, days_ago, now_iso, plural_units, qty_label};

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
