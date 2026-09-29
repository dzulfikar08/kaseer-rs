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
    ActiveTheme as _, Disableable as _, Icon, IndexPath, Sizable as _, WindowExt,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::prelude::StyledImage as _;
use gpui_kit::{
    Anchor, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, img, px,
};
use kaseer_core::cart::{self, Cart, CartTotals, Discount};
use kaseer_core::currency::{Currency, IDR, currency_by_code};
use kaseer_core::db::{open_repo, resolve_db_path};
use kaseer_core::escpos::escpos_bytes;
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
        screen
    }

    // ── Data flow ─────────────────────────────────────────────────────────

    fn totals(&self) -> CartTotals {
        cart::compute_totals(&self.cart, &self.tax_rates, self.currency)
    }

    /// Live filter over the search box: category chip plus a
    /// name-or-SKU substring match (the web grid's `useMemo`).
    fn filtered_products(&self, cx: &Context<Self>) -> Vec<Product> {
        let query = self.query.read(cx).value().trim().to_lowercase();
        self.products
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
            .collect()
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
        self.refresh_catalog(window, cx);
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

    fn load_stock(&self) -> HashMap<String, i64> {
        stock_map(&self.repo, &self.products)
    }

    /// Menu pull watch: the pull engine writes the ledger on a background
    /// thread and records the applied revision in a process global. The
    /// tick itself is deliberately trivial (a field write + notify, the
    /// same shape as Settings' refresh loop) — an async entity update must
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
    /// pass — one owner, one thread, no re-entrant entity updates. A failed
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

    fn render_catalog(&self, cx: &Context<Self>, products: &[Product]) -> impl IntoElement {
        v_flex()
            .flex_grow(1.6)
            .min_w_0()
            .gap_2()
            // The input's editor renders size_full — placed bare in a flex
            // column it would take the whole column height and starve the
            // grid. The fixed-height wrapper pins it to one line.
            .child(
                div()
                    .h(px(38.))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .child(Input::new(&self.query).id("search")),
            )
            .child(self.render_category_chips(cx))
            .child(self.render_product_grid(cx, products))
    }

    fn render_category_chips(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut row = h_flex()
            .gap(px(8.))
            .flex_none()
            .flex_shrink_0()
            .h(px(34.))
            .overflow_x_scrollbar();
        row = row.child(super::chip(
            cx,
            "cat-all",
            "All",
            self.category.is_none(),
            cx.listener(|this, _, _, cx| {
                this.category = None;
                cx.notify();
            }),
        ));
        for name in self.categories.clone() {
            row = row.child(super::chip(
                cx,
                SharedString::from(format!("cat-{name}")),
                SharedString::from(name.clone()),
                self.category.as_deref() == Some(name.as_str()),
                cx.listener(move |this, _, _, cx| {
                    this.category = Some(name.clone());
                    cx.notify();
                }),
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
        let mut grid = div()
            .grid()
            .grid_cols(4)
            .gap(px(10.))
            .flex_grow(1.)
            .min_h_0()
            .overflow_y_scrollbar();
        for product in products {
            grid = grid.child(self.render_product_card(product, cx));
        }
        grid.into_any_element()
    }

    fn render_product_card(&self, product: &Product, cx: &Context<Self>) -> impl IntoElement {
        let stock_note = self
            .stock
            .get(&product.id)
            .map(|on_hand| on_hand.to_string());
        let known = has_knowledge(product);
        // Photo band (the web `.band`): pulled from the companion into the
        // ledger's images/ dir. An empty path or a not-yet-downloaded file
        // keeps the text-only card until a later pull lands the file.
        let photo = menu::local_image_path(self.repo.db_path(), &product.image_path)
            .filter(|path| path.exists());
        let body = v_flex()
            .flex_grow(1.)
            .gap(px(4.))
            .p(px(12.))
            // The web card reserves the corner for the knowledge affordance
            // (`paddingRight: 28`) so long names never run under it.
            .when(known, |body| body.pr(px(28.)))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    // Two lines before the ellipsis: single-line truncation
                    // shredded every long name at narrow grid widths.
                    .line_clamp(2)
                    .child(SharedString::from(product.name.clone())),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .truncate()
                    .child(SharedString::from(product.sku.clone())),
            )
            .when_some(stock_note, |body, note| {
                // Stock lives on its own line so the count can never read
                // as extra digits of the price.
                body.child(
                    div()
                        .mt_1()
                        .text_size(px(10.5))
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(format!("In stock · {note}"))),
                )
            })
            .child(
                h_flex()
                    .mt_auto()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_family(crate::theme::FONT_MONO)
                            .font_features(crate::theme::tabular())
                            .text_color(cx.theme().accent)
                            .child(SharedString::from(fmt_money(
                                product.price_minor,
                                self.currency,
                            ))),
                    ),
            );
        let card = v_flex()
            .id(SharedString::from(format!("product-{}", product.id)))
            .cursor_pointer()
            .min_h(px(86.))
            // The band bleeds to the card edges (web `margin: 0 -12px`):
            // clip it to the rounded card instead.
            .overflow_hidden()
            .rounded(crate::theme::CARD_RADIUS)
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .hover(|style| style.border_color(crate::theme::accent_dim()))
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
            .when_some(photo, |card, path| {
                card.child(
                    img(path)
                        .id(SharedString::from(format!("product-img-{}", product.id)))
                        .w_full()
                        .h(px(96.))
                        .flex_none()
                        .object_fit(ObjectFit::Cover),
                )
            })
            .child(body);
        if known {
            // The web `.kinfo`: a small corner button floating over the
            // card's top-right, opening a click-away knowledge popover.
            // The position lives on a wrapper div — the Popover styles its
            // content surface, not the trigger.
            div()
                .relative()
                .child(card)
                .child(
                    div()
                        .absolute()
                        .top(px(6.))
                        .right(px(6.))
                        .child(self.render_knowledge_popover(product, cx)),
                )
                .into_any_element()
        } else {
            card.into_any_element()
        }
    }

    /// The knowledge affordance: an Info corner button floating over the
    /// card's top-right (the web `.kinfo` position), opening a click-away
    /// knowledge popover — the register.tsx `openPeek` flow on gpui-kit's
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
    ) -> impl IntoElement {
        v_flex()
            .flex_grow(1.)
            // Let the panel shrink with the window row instead of sizing to
            // its content: unconstrained, the lines list keeps its content
            // height and its scroll hitbox spills over totals/pay.
            .min_h_0()
            .overflow_hidden()
            .max_w(px(420.))
            .min_w_0()
            .gap(px(10.))
            .p(px(14.))
            .rounded(crate::theme::CARD_RADIUS)
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(self.render_cart_header(cx))
            .when_some(customer, |panel, customer| {
                panel.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(format!(
                            "{} — {} pts",
                            customer.name, customer.points
                        ))),
                )
            })
            .child(self.render_cart_lines(cx))
            .child(self.render_discount_row(cx))
            .child(self.render_totals_block(cx, totals))
            .child(self.render_pay_button(cx, totals))
    }

    fn render_cart_header(&self, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .justify_between()
            .items_center()
            .gap_2()
            .flex_none()
            .child(
                // The web cart header is the panel kicker: uppercase 12px,
                // letter-spaced, muted. It ellipsizes instead of colliding
                // with the customer select at narrow widths.
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(format!(
                        "CURRENT SALE — {} ITEMS",
                        self.cart.lines.len()
                    ))),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(150.))
                    .child(Select::new(&self.customer_select)),
            )
    }

    fn render_cart_lines(&self, cx: &Context<Self>) -> impl IntoElement {
        if self.cart.lines.is_empty() {
            return v_flex()
                .flex_grow(1.)
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_2()
                .py_8()
                .child(
                    Icon::new(IconName::Scan)
                        .with_size(px(26.))
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Cart is empty."),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(cx.theme().muted_foreground)
                        .opacity(0.8)
                        .child("Scan a barcode or tap a product to start the sale."),
                )
                .into_any_element();
        }
        let mut list = v_flex()
            .flex_grow(1.)
            .min_h_0()
            .gap_2()
            .overflow_y_scrollbar();
        for line in &self.cart.lines {
            list = list.child(self.render_cart_line(line, cx));
        }
        list.into_any_element()
    }

    fn render_cart_line(&self, line: &cart::CartLine, cx: &Context<Self>) -> impl IntoElement {
        let gross_minor = (line.unit_minor as f64 * line.qty).round() as i64;
        h_flex()
            .justify_between()
            .gap_2()
            .pb_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(px(13.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(SharedString::from(line.name.clone())),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(format!(
                                "{} × {}",
                                fmt_money(line.unit_minor, self.currency),
                                line.qty
                            ))),
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
                    .child(
                        div()
                            .w(px(74.))
                            .text_right()
                            .text_sm()
                            .font_family(crate::theme::FONT_MONO)
                            .font_features(crate::theme::tabular())
                            .child(SharedString::from(fmt_money(gross_minor, self.currency))),
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

    fn render_total_row(
        &self,
        label: &str,
        value: String,
        color: Hsla,
        _cx: &Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .justify_between()
            .text_sm()
            .child(div().text_color(color).child(SharedString::from(label)))
            .child(
                div()
                    .font_family(crate::theme::FONT_MONO)
                            .font_features(crate::theme::tabular())
                    .text_color(color)
                    .child(SharedString::from(value)),
            )
    }

    fn render_totals_block(&self, cx: &Context<Self>, totals: &CartTotals) -> impl IntoElement {
        v_flex()
            .gap_1()
            .pt_2()
            .flex_none()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(self.render_total_row(
                "Subtotal",
                fmt_money(totals.gross_minor, self.currency),
                cx.theme().muted_foreground,
                cx,
            ))
            .when(totals.discount_minor > 0, |block| {
                block.child(self.render_total_row(
                    "Discount",
                    format!("-{}", fmt_money(totals.discount_minor, self.currency)),
                    cx.theme().accent,
                    cx,
                ))
            })
            .child(self.render_total_row(
                "Tax",
                fmt_money(totals.tax_minor, self.currency),
                cx.theme().muted_foreground,
                cx,
            ))
            .child(
                h_flex()
                    .justify_between()
                    .pt_1()
                    .child(div().text_lg().font_semibold().child("TOTAL"))
                    .child(
                        div()
                            .text_lg()
                            .font_semibold()
                            .font_family(crate::theme::FONT_MONO)
                            .font_features(crate::theme::tabular())
                            .child(SharedString::from(fmt_money(
                                totals.total_minor,
                                self.currency,
                            ))),
                    ),
            )
    }

    fn render_pay_button(&self, cx: &Context<Self>, totals: &CartTotals) -> impl IntoElement {
        let payable = !self.cart.lines.is_empty() && self.active_shift.is_some();
        let label = match &self.active_shift {
            Some(_) => format!("Pay — {}", fmt_money(totals.total_minor, self.currency)),
            None => "Open a shift to sell".to_string(),
        };
        // The kit's disabled primary drops to 15% opacity, which reads as a
        // rendering glitch rather than a locked control. Render the locked
        // state ourselves: visible surface, explicit hint, no ambiguity.
        if payable {
            Button::new("pay")
                .primary()
                .large()
                .w_full()
                .label(SharedString::from(label))
                .on_click(cx.listener(|this, _, window, cx| this.open_pay(window, cx)))
                .into_any_element()
        } else {
            v_flex()
                .id("pay-locked")
                .w_full()
                .gap_1()
                .items_center()
                .py(px(12.))
                .rounded(cx.theme().radius)
                .bg(cx.theme().secondary)
                .border_1()
                .border_color(cx.theme().border)
                .text_color(cx.theme().muted_foreground)
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(SharedString::from(label)),
                )
                .child(
                    div().text_size(px(11.)).child(match &self.active_shift {
                        Some(_) => "Add items to the sale to enable payment",
                        None => "Open a shift from the Shift screen first",
                    }),
                )
                .into_any_element()
        }
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
        let totals = self.totals();
        let products = self.filtered_products(cx);
        let customer = self.attached_customer();

        h_flex()
            .size_full()
            .items_stretch()
            .gap(px(10.))
            .p_4()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_catalog(cx, &products))
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
            .child(
                v_flex().gap_0p5().child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("TOTAL"),
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
                            .font_family(crate::theme::FONT_MONO)
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
                    .font_family(crate::theme::FONT_MONO)
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
                                    .font_family(crate::theme::FONT_MONO)
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
            // Web `.numgrid button`: 14px padding, 16px text — a ~48px key.
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
        h_flex()
            .gap_3()
            .items_center()
            .child(
                Button::new("pay-cancel")
                    .ghost()
                    .label("Cancel (Esc)")
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            )
            .child(grid)
            .child(
                Button::new("pay-paid")
                    .primary()
                    .large()
                    .self_end()
                    .label("Paid")
                    .disabled(!complete)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.emit(PayEvent::Paid {
                            tenders: this.tenders.clone(),
                            redeem_points,
                        });
                    })),
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

    /// The shown sale, if any — the training screen re-opens the paper for
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
                    .font_family(crate::theme::FONT_MONO)
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
/// filled — the web `hasKnowledge` contract decides the corner affordance.
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
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .child(SharedString::from(product.name.clone())),
        )
        .when(!product.description.is_empty(), |card| {
            card.child(
                div()
                    .text_size(px(12.5))
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
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted)
                            .child("PREP"),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
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
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted)
                            .child("ALLERGENS"),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .child(SharedString::from(product.allergens.clone())),
                    ),
            )
        })
}

pub(crate) fn fmt_money(minor: i64, currency: &Currency) -> String {
    money::format(&money::money(minor, currency.code), Some(currency)).unwrap_or_default()
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
    format!(
        "{}{:03}Z",
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
    use super::civil_timestamp;

    #[test]
    fn civil_timestamp_matches_known_instants() {
        assert_eq!(civil_timestamp(0), "1970-01-01T00:00:00");
        assert_eq!(civil_timestamp(-1), "1969-12-31T23:59:59");
        // Leap day and a recent known instant.
        assert_eq!(civil_timestamp(951_782_400), "2000-02-29T00:00:00");
        assert_eq!(civil_timestamp(1_700_000_000), "2023-11-14T22:13:20");
    }
}
