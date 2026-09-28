//! Training mode: an 8-lesson guided tour of the register flow on a
//! sandbox ledger. "Start training" copies the live ledger (or seeds a
//! fresh one) to `training.db` next to it; every lesson action — shift,
//! cart, payment, receipt, close — lands in that copy, so a trainee's
//! sales never touch the real ledger. Lesson progress persists in the
//! sandbox's settings `config_json`, and validation reads the sandbox
//! through the same repo seams the real screens use
//! (`active_shift`, `shift_sales`, `sale_by_id`). The pay, receipt, and
//! close-shift dialogs are the register's own components, reused verbatim.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::base::{StyledExt as _, h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IndexPath, Sizable as _, WindowExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, px,
};
use kaseer_core::cart::{self, Cart, CartTotals, Discount};
use kaseer_core::currency::{Currency, IDR, currency_by_code};
use kaseer_core::db::{open_repo, resolve_db_path, snapshot_db};
use kaseer_core::loyalty::{max_redeemable_points, points_earned, redeem_value_minor};
use kaseer_core::payments::{self, Tender, TenderMethod};
use kaseer_core::receipt::{Sale, SaleLine, SaleTender, StoreInfo};
use kaseer_core::repos::{Customer, Product, Repo, Settings};
use kaseer_core::shift::{Shift, cash_expected_minor};
use kaseer_core::tax::TaxRate;

use super::register::{
    CustomerItem, PayDialog, PayEvent, ReceiptDialog, ReceiptEvent, customer_items, fmt_money,
};
use super::shift::CloseShiftDialog;

const CUR_FALLBACK: &Currency = &IDR;

/// Opening float the first lesson asks for.
const LESSON_FLOAT_MINOR: i64 = 100_000;
/// Order discount the fifth lesson asks for, in basis points.
const LESSON_DISCOUNT_BPS: i64 = 1_000;

/// The 8 lessons, in order: number, title, instruction. Validation lives in
/// `lesson_complete`, keyed by the same numbers.
const LESSONS: [(u8, &str, &str); 8] = [
    (
        1,
        "Open a shift",
        "Use the quick Rp 100.000 float (or type 100000), then press Open shift. The drawer lives in the sandbox ledger.",
    ),
    (
        2,
        "Scan a product by SKU",
        "Type the Americano's SKU 8991002100028 into the scan box and press Enter. An exact SKU adds the product to the cart.",
    ),
    (
        3,
        "Adjust the quantity",
        "Press + on the cart line to make it 2, then − to come back to 1. Watch the total follow the quantity.",
    ),
    (
        4,
        "Attach a customer",
        "Pick any customer from the select on this card (Walk-in means nobody).",
    ),
    (
        5,
        "Give an order discount",
        "Type 10 into the percent box and press Apply % — 10% off the whole order.",
    ),
    (
        6,
        "Take a cash payment with change",
        "Press Pay in the cart panel, hit the Rp 20.000 quick note, then Paid. The total is Rp 15.000, so the trainee owes Rp 5.000 change.",
    ),
    (
        7,
        "Read the receipt",
        "The receipt is on screen. Read it — lines, change, shift — then press New sale to close it.",
    ),
    (
        8,
        "Close the shift with a count",
        "Press Close shift…, count the drawer to exactly the expected cash (one Rp 100.000 note), then Confirm close.",
    ),
];

pub struct TrainingScreen {
    session: Option<Session>,
    float: Entity<InputState>,
    scan: Entity<InputState>,
    discount: Entity<InputState>,
    customer_select: Entity<SelectState<Vec<CustomerItem>>>,
    pay_dialog: Entity<PayDialog>,
    receipt_dialog: Entity<ReceiptDialog>,
    _subscriptions: Vec<Subscription>,
}

/// One training run against one sandbox ledger.
struct Session {
    repo: Repo,
    sandbox_path: PathBuf,
    cur: &'static Currency,
    settings: Settings,
    tax_rates: Vec<TaxRate>,
    products: Vec<Product>,
    customers: Vec<Customer>,
    /// Completed lesson numbers, mirrored from the sandbox `config_json`.
    completed: Vec<u8>,
    /// Highest quantity any cart line reached this session (lesson 3).
    max_line_qty: f64,
    /// The pay flow opened the receipt (lesson 7 waits for its close).
    receipt_shown: bool,
    receipt_closed: bool,
    cart: Cart,
}

impl TrainingScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let float = cx.new(|cx| InputState::new(window, cx).placeholder("Opening float (Rp)"));
        let scan =
            cx.new(|cx| InputState::new(window, cx).placeholder("Type a SKU and press Enter"));
        let discount = cx.new(|cx| InputState::new(window, cx).placeholder("%"));
        let customer_select =
            cx.new(|cx| SelectState::new(Vec::new(), Some(IndexPath::new(0)), window, cx));
        let pay_dialog = cx.new(|cx| PayDialog::new(CUR_FALLBACK, window, cx));
        let receipt_dialog = cx.new(|_cx| ReceiptDialog::new(None, CUR_FALLBACK));

        let mut screen = Self {
            session: None,
            float,
            scan,
            discount,
            customer_select,
            pay_dialog,
            receipt_dialog,
            _subscriptions: Vec::new(),
        };
        screen.wire_subscriptions(window, cx);
        screen
    }

    fn wire_subscriptions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let scan = self.scan.clone();
        self._subscriptions.push(
            cx.subscribe_in(&scan, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.scan_sku(window, cx);
                }
            }),
        );
        let float = self.float.clone();
        self._subscriptions.push(
            cx.subscribe_in(&float, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.open_shift_from_input(window, cx);
                }
            }),
        );
        let select = self.customer_select.clone();
        self._subscriptions
            .push(
                cx.subscribe_in(&select, window, |this, _, event, _, cx| match event {
                    SelectEvent::Confirm(value) => {
                        let id = value.clone().filter(|id| !id.is_empty());
                        if let Some(session) = &mut this.session {
                            session.cart = cart::set_cart_customer(&session.cart, id);
                            this.refresh(cx);
                        }
                    }
                }),
            );
        let pay = self.pay_dialog.clone();
        self._subscriptions
            .push(cx.subscribe_in(&pay, window, |this, _, event, window, cx| {
                this.on_pay_event(event, window, cx)
            }));
        let receipt = self.receipt_dialog.clone();
        self._subscriptions
            .push(cx.subscribe_in(&receipt, window, |this, _, event, _, cx| {
                if matches!(event, ReceiptEvent::Closed) {
                    if let Some(session) = &mut this.session {
                        session.receipt_closed = session.receipt_shown;
                    }
                    this.refresh(cx);
                }
            }));
    }

    // ── Session lifecycle ─────────────────────────────────────────────────

    /// Copy the live ledger into the sandbox (or seed a fresh one) and open
    /// it. A stale sandbox is removed first — `VACUUM INTO` needs an
    /// absent target, and Reset means a fresh ledger.
    fn start_training(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match Session::start() {
            Ok(mut session) => {
                let items = customer_items(&session.customers);
                let float_text = String::new();
                session.cart = cart::empty_cart(session.cur.code);
                self.customer_select.update(cx, |select, cx| {
                    select.set_items(items, window, cx);
                    select.set_selected_index(Some(IndexPath::new(0)), window, cx);
                });
                for input in [&self.float, &self.scan, &self.discount] {
                    input.update(cx, |input, cx| {
                        input.set_value(float_text.clone(), window, cx)
                    });
                }
                self.receipt_dialog.update(cx, |dialog, _| dialog.clear());
                window.push_notification(
                    Notification::success(format!(
                        "Training sandbox ready: {}",
                        session.sandbox_path.display()
                    )),
                    cx,
                );
                self.session = Some(session);
                cx.notify();
            }
            Err(error) => window.push_notification(
                Notification::error(format!("Could not start training: {error}")),
                cx,
            ),
        }
    }

    /// Reset = start over: a fresh sandbox copy and cleared progress.
    fn reset_training(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session = None;
        self.start_training(window, cx);
    }

    // ── Lesson actions ────────────────────────────────────────────────────

    fn scan_sku(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        let term = self.scan.read(cx).value().trim().to_lowercase();
        let Some(product) = session
            .products
            .iter()
            .find(|p| p.sku.to_lowercase() == term)
            .cloned()
        else {
            window.push_notification(
                Notification::error("No product with that exact SKU".to_string()),
                cx,
            );
            return;
        };
        match cart::add_to_cart(&session.cart, &product.addable(), 1.0) {
            Ok(next) => session.cart = next,
            Err(error) => window.push_notification(Notification::error(error.to_string()), cx),
        }
        self.scan
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.refresh(cx);
    }

    fn set_line_qty(&mut self, line_id: &str, qty: f64, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        if let Ok(next) = cart::set_line_qty(&session.cart, line_id, qty) {
            session.cart = next;
            session.max_line_qty = session.max_line_qty.max(qty);
            self.refresh(cx);
        }
    }

    fn remove_line(&mut self, line_id: &str, cx: &mut Context<Self>) {
        if let Some(session) = &mut self.session {
            session.cart = cart::remove_line(&session.cart, line_id);
            self.refresh(cx);
        }
    }

    fn open_shift_from_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        let raw = self.float.read(cx).value().trim().to_string();
        let Ok(minor) = kaseer_core::money::parse(&raw, session.cur).map(|m| m.minor) else {
            return;
        };
        match session.repo.open_shift(minor, session.cur.code) {
            Ok(_) => {
                self.float
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.refresh(cx);
            }
            Err(error) => window.push_notification(
                Notification::error(format!("Could not open the shift: {error}")),
                cx,
            ),
        }
    }

    fn apply_order_percent(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        let raw = self.discount.read(cx).value().trim().to_string();
        let bps = raw
            .parse::<f64>()
            .map(|value| (value * 100.0).round() as i64)
            .unwrap_or(0);
        session.cart = cart::set_order_discount(
            &session.cart,
            (bps > 0).then_some(Discount::Percent { bps }),
        );
        self.refresh(cx);
    }

    fn clear_order_percent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        session.cart = cart::set_order_discount(&session.cart, None);
        self.discount
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.refresh(cx);
    }

    // ── Pay flow ──────────────────────────────────────────────────────────

    fn totals(&self) -> Option<CartTotals> {
        let session = self.session.as_ref()?;
        Some(cart::compute_totals(
            &session.cart,
            &session.tax_rates,
            session.cur,
        ))
    }

    fn attached_customer(&self) -> Option<Customer> {
        let session = self.session.as_ref()?;
        let id = session.cart.customer_id.as_ref()?;
        session.customers.iter().find(|c| c.id == *id).cloned()
    }

    fn open_pay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        let Some(totals) = self.totals() else {
            return;
        };
        let customer = self
            .attached_customer()
            .map(|c| super::register::CustomerSnapshot {
                id: c.id,
                name: c.name,
                points: c.points,
            });
        let cur = session.cur;
        self.pay_dialog.update(cx, |dialog, cx| {
            dialog.reset(
                totals.total_minor,
                customer,
                session.settings.loyalty.clone(),
                cur,
                window,
                cx,
            )
        });
        let pay = self.pay_dialog.clone();
        let surface = cx.theme().popover;
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Payment — training sandbox")
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
        match self.checkout(tenders, *redeem_points) {
            Ok(sale) => {
                window.close_dialog(cx);
                self.open_receipt(sale, window, cx);
            }
            Err(message) => window.push_notification(Notification::error(message), cx),
        }
        self.refresh(cx);
    }

    /// Settle the training sale into the sandbox, mirroring the register's
    /// checkout transaction exactly (`Repo::insert_sale` in one write).
    fn checkout(&mut self, tenders: &[Tender], redeem_points: i64) -> Result<Sale, String> {
        let (shift, totals, loyalty) = {
            let Some(session) = &self.session else {
                return Err("Training has not started".to_string());
            };
            let Some(shift) = session.repo.active_shift().map_err(|e| e.to_string())? else {
                return Err("No open shift. Open one first.".to_string());
            };
            (
                shift,
                self.totals().ok_or("no cart")?,
                session.settings.loyalty.clone(),
            )
        };
        let session = self.session.as_ref().expect("checked above");
        let customer = self.attached_customer();
        let redeemable = redeem_points.max(0).min(max_redeemable_points(
            customer.as_ref().map(|c| c.points).unwrap_or(0),
            totals.net_minor,
            &loyalty,
        ));
        let redeem_value = redeem_value_minor(redeemable, &loyalty);
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

        let earned = points_earned(totals.net_minor, &loyalty);
        let (number, _seq) = session.repo.next_sale_number().map_err(|e| e.to_string())?;

        let lines: Vec<SaleLine> = totals
            .lines
            .iter()
            .zip(&session.cart.lines)
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
            id: format!("sale-{}", super::register::unix_nanos()),
            number,
            at: super::register::now_iso(),
            currency: session.cur.code.to_string(),
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

        let mut tracked = std::collections::HashMap::new();
        for line in &session.cart.lines {
            if let Some(product) = session.products.iter().find(|p| p.id == line.product_id)
                && product.track_stock
            {
                *tracked.entry(line.product_id.clone()).or_insert(0.0) += line.qty.round();
            }
        }

        session
            .repo
            .insert_sale(&sale, &tracked, earned - redeemable)
            .map_err(|error| error.to_string())?;

        let session = self.session.as_mut().expect("checked above");
        session.cart = cart::clear_cart(&session.cart);
        Ok(sale)
    }

    fn open_receipt(&mut self, sale: Sale, window: &mut Window, cx: &mut Context<Self>) {
        let store = StoreInfo {
            name: self
                .session
                .as_ref()
                .map(|s| s.settings.store_name.clone())
                .unwrap_or_else(|| "Kaseer Training".to_string()),
            address: None,
            footer: None,
        };
        let number = sale.number.clone();
        self.receipt_dialog
            .update(cx, |dialog, _| dialog.show(sale, store));
        if let Some(session) = &mut self.session {
            session.receipt_shown = true;
            session.receipt_closed = false;
        }
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

    /// Lesson 7 kindness: re-open the last receipt if it was closed early.
    fn show_receipt_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sale) = self.receipt_dialog.read(cx).sale() else {
            return;
        };
        self.open_receipt(sale, window, cx);
    }

    // ── Close shift ───────────────────────────────────────────────────────

    fn open_close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        let Some(shift) = session.repo.active_shift().ok().flatten() else {
            return;
        };
        let sales = session.repo.shift_sales(&shift.id).unwrap_or_default();
        let expected = cash_expected_minor(&shift, &sales);
        let cur = session.cur;
        let screen = cx.entity();

        // Built once here; the dialog-layer builder below only re-wraps it,
        // or the count inputs would reset per frame (the shift screen's
        // documented dialog pattern).
        let view = cx.new(|cx| CloseShiftDialog::new(expected, cur, window, cx));

        let surface = cx.theme().popover;
        window.open_dialog(cx, move |dialog, _, _| {
            let ok_view = view.clone();
            let ok_screen = screen.clone();
            dialog
                .title("Close shift — count the drawer (training)")
                .w(px(560.))
                .bg(surface)
                .child(view.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .trigger(|button| button.label("Cancel").outline().small()),
                        )
                        .child(
                            DialogAction::new()
                                .child(Button::new("ok").label("Confirm close").primary().small()),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let counted = ok_view.read(cx).counted_minor(cx);
                    ok_screen
                        .update(cx, |screen, cx| screen.close_shift(counted, window, cx))
                        .inspect_err(|message| {
                            window.push_notification(Notification::error(message.clone()), cx)
                        })
                        .is_ok()
                })
        });
    }

    /// `Ok` lets the dialog layer close; `Err` keeps it open, mirroring the
    /// shift screen's close flow.
    fn close_shift(
        &mut self,
        counted_minor: i64,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(session) = &mut self.session else {
            return Err("Training has not started".to_string());
        };
        let Some(shift) = session.repo.active_shift().ok().flatten() else {
            return Err("No shift is open".to_string());
        };
        session
            .repo
            .close_shift(&shift.id, counted_minor)
            .map_err(|error| format!("Could not close the shift: {error}"))?;
        self.refresh(cx);
        Ok(())
    }

    // ── Validation ────────────────────────────────────────────────────────

    /// The validation cycle: re-read the sandbox through the same repo seams
    /// the screens use, complete every lesson whose predicate now holds
    /// (persisting the progress into the sandbox's `config_json`), and
    /// refresh the UI.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else {
            return;
        };
        let state = LessonState::observe(session);
        let mut completed = session.progress();
        let mut changed = false;
        for (number, _, _) in LESSONS {
            if completed.contains(&number) {
                continue;
            }
            if lesson_complete(number, session, &state) {
                completed.push(number);
                changed = true;
            }
        }
        if changed {
            completed.sort_unstable();
            completed.dedup();
            if let Err(error) = session.store_progress(&completed) {
                // Progress is a persistence nicety; the run continues.
                eprintln!("training: could not persist progress: {error}");
            }
        }
        session.completed = completed;
        cx.notify();
    }

    fn current_lesson(&self) -> u8 {
        let session = self.session.as_ref();
        let completed = session.map(|s| s.completed.as_slice()).unwrap_or(&[]);
        LESSONS
            .iter()
            .find(|(number, _, _)| !completed.contains(number))
            .map(|(number, _, _)| *number)
            .unwrap_or(LESSONS.len() as u8 + 1)
    }
}

impl Session {
    /// Copy the live ledger (or seed fresh) into `training.db`, then open
    /// it as the sandbox.
    fn start() -> Result<Session, String> {
        let source = resolve_db_path();
        let parent = Path::new(&source)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let sandbox_path = parent.join("training.db");
        remove_sandbox(&sandbox_path)
            .map_err(|error| format!("could not clear the old sandbox: {error}"))?;
        if Path::new(&source).exists() {
            snapshot_db(&source, sandbox_path.to_str().expect("utf-8 sandbox path"))
                .map_err(|error| format!("could not copy the ledger: {error}"))?;
        }
        // A missing source seeds a brand-new ledger; either way the sandbox
        // is migrated and demo-seeded by open_repo.
        let repo = open_repo(sandbox_path.to_str().expect("utf-8 sandbox path"))
            .map_err(|error| format!("could not open the sandbox: {error}"))?;
        let settings = repo.settings().map_err(|e| e.to_string())?;
        let cur = currency_by_code(&settings.currency_code).unwrap_or(CUR_FALLBACK);
        let tax_rates = repo.tax_rates().map_err(|e| e.to_string())?;
        let products = repo.products(false, None).map_err(|e| e.to_string())?;
        let customers = repo.customers(None).map_err(|e| e.to_string())?;
        let mut session = Session {
            repo,
            sandbox_path,
            cur,
            settings,
            tax_rates,
            products,
            customers,
            completed: Vec::new(),
            max_line_qty: 1.0,
            receipt_shown: false,
            receipt_closed: false,
            cart: cart::empty_cart(cur.code),
        };
        session.completed = session.progress();
        Ok(session)
    }

    /// Completed lesson numbers, read back from the sandbox's settings
    /// `config_json` — the same blob the web companion keeps its store
    /// config in; training parks its progress under a `training` key the
    /// companion ignores.
    fn progress(&self) -> Vec<u8> {
        let Ok(raw): Result<String, _> =
            self.repo
                .conn()
                .query_row("SELECT config_json FROM settings WHERE id = 1", [], |r| {
                    r.get(0)
                })
        else {
            return vec![];
        };
        parse_progress(&raw)
    }

    fn store_progress(&self, completed: &[u8]) -> Result<(), String> {
        let raw: String = self
            .repo
            .conn()
            .query_row("SELECT config_json FROM settings WHERE id = 1", [], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
        let mut config: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if !config.is_object() {
            config = serde_json::json!({});
        }
        config["training"] = serde_json::json!({ "completed": completed });
        self.repo
            .conn()
            .execute(
                "UPDATE settings SET config_json = ? WHERE id = 1",
                [config.to_string()],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Parse the sandbox progress: `{"training":{"completed":[1,2]}}` → [1, 2].
fn parse_progress(raw: &str) -> Vec<u8> {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|config| {
            config
                .get("training")
                .and_then(|training| training.get("completed"))
                .and_then(|completed| completed.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|v| v.as_u64().map(|n| n as u8))
                        .collect()
                })
        })
        .unwrap_or_default()
}

/// Remove the sandbox plus its WAL sidecars; missing files are fine.
fn remove_sandbox(path: &Path) -> std::io::Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let mut candidate = path.as_os_str().to_owned();
        candidate.push(suffix);
        match std::fs::remove_file(&candidate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Everything one validation pass needs from the sandbox, reduced once.
struct LessonState {
    active_shift: Option<Shift>,
    last_sale: Option<Sale>,
    closed_shift: Option<Shift>,
    closed_expected_cash: i64,
}

impl LessonState {
    fn observe(session: &Session) -> LessonState {
        let active_shift = session.repo.active_shift().ok().flatten();
        let last_sale = last_sale(&session.repo);
        let closed_shift = last_closed_shift(&session.repo);
        let closed_expected_cash = match &closed_shift {
            Some(shift) => session
                .repo
                .shift_sales(&shift.id)
                .ok()
                .map(|sales| cash_expected_minor(shift, &sales))
                .unwrap_or(0),
            None => 0,
        };
        LessonState {
            active_shift,
            last_sale,
            closed_shift,
            closed_expected_cash,
        }
    }
}

/// Newest sale in the sandbox (rowid breaks `at` ties within a millis).
fn last_sale(repo: &Repo) -> Option<Sale> {
    let id: String = repo
        .conn()
        .query_row(
            "SELECT id FROM sales ORDER BY at DESC, rowid DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok()?;
    repo.sale_by_id(&id).ok().flatten()
}

/// Most recently closed shift, for the lesson-8 count check.
fn last_closed_shift(repo: &Repo) -> Option<Shift> {
    repo.conn()
        .query_row(
            "SELECT id, opened_at, opening_float_minor, currency, closed_at, counted_minor
             FROM shifts WHERE closed_at IS NOT NULL ORDER BY closed_at DESC LIMIT 1",
            [],
            |r| {
                Ok(Shift {
                    id: r.get(0)?,
                    opened_at: r.get(1)?,
                    opening_float_minor: r.get(2)?,
                    currency: r.get(3)?,
                    closed_at: r.get(4)?,
                    counted_minor: r.get(5)?,
                })
            },
        )
        .ok()
}

/// The live predicate for one lesson. Everything reads the sandbox ledger
/// or the session cart — never screen-only state that could lie.
fn lesson_complete(number: u8, session: &Session, state: &LessonState) -> bool {
    match number {
        1 => state
            .active_shift
            .as_ref()
            .is_some_and(|shift| shift.opening_float_minor == LESSON_FLOAT_MINOR),
        2 => !session.cart.lines.is_empty(),
        3 => session.max_line_qty >= 2.0 && session.cart.lines.iter().any(|line| line.qty == 1.0),
        4 => session.cart.customer_id.is_some(),
        5 => {
            session.cart.order_discount
                == Some(Discount::Percent {
                    bps: LESSON_DISCOUNT_BPS,
                })
        }
        6 => state.last_sale.as_ref().is_some_and(|sale| {
            sale.change_minor > 0
                && sale.tenders.iter().any(|tender| {
                    tender.method == TenderMethod::Cash
                        && tender.given_minor.unwrap_or(0) > tender.applied_minor
                })
        }),
        7 => session.receipt_closed && state.last_sale.is_some(),
        8 => state.closed_shift.as_ref().is_some_and(|shift| {
            shift
                .counted_minor
                .is_some_and(|counted| counted > 0 && counted == state.closed_expected_cash)
        }),
        _ => false,
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────

impl Render for TrainingScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("training-screen")
            .size_full()
            .overflow_y_scrollbar()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(match &self.session {
                None => self.render_launcher(cx).into_any_element(),
                Some(_) => self.render_session(cx).into_any_element(),
            })
    }
}

impl TrainingScreen {
    /// No session: a centered launcher card explaining the sandbox.
    fn render_launcher(&self, cx: &Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(480.))
                    .gap_4()
                    .p_8()
                    .m_6()
                    .rounded(crate::theme::CARD_RADIUS)
                    .bg(cx.theme().secondary)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Icon::new(IconName::GraduationCap)
                                    .with_size(px(22.))
                                    .text_color(cx.theme().accent),
                            )
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Training"),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(format!(
                                "Copies the live ledger to {} and runs {} guided lessons against \
                                 the copy — trainee sales never touch the real ledger.",
                                sandbox_display_path(),
                                LESSONS.len(),
                            ))),
                    )
                    .child(
                        Button::new("start-training")
                            .primary()
                            .large()
                            .w_full()
                            .label("Start training")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_training(window, cx);
                            })),
                    ),
            )
    }

    /// The active session: banner, progress, lesson card, checklist, cart.
    fn render_session(&self, cx: &Context<Self>) -> impl IntoElement {
        let completed_count = self
            .session
            .as_ref()
            .map(|s| s.completed.len())
            .unwrap_or(0);
        let all_done = completed_count >= LESSONS.len();
        let current = self.current_lesson();

        v_flex()
            .w_full()
            .max_w(px(1100.))
            .mx_auto()
            .p_6()
            .gap_4()
            // The sandbox banner: unmistakable, always first.
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .px_3()
                    .py_2()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(crate::theme::accent_dim())
                    .bg(cx.theme().muted)
                    .child(
                        Icon::new(IconName::GraduationCap)
                            .with_size(px(16.))
                            .text_color(cx.theme().accent),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().accent)
                            .child("TRAINING — sandbox ledger"),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(sandbox_display_path())),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("reset-training")
                            .outline()
                            .small()
                            .label("Reset training")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.reset_training(window, cx);
                            })),
                    ),
            )
            .child(self.render_progress(cx, completed_count, all_done))
            .when(all_done, |body| {
                body.child(
                    v_flex()
                        .gap_1()
                        .p(px(14.))
                        .rounded(crate::theme::CARD_RADIUS)
                        .border_1()
                        .border_color(cx.theme().success)
                        .bg(cx.theme().muted)
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    Icon::new(IconName::CircleCheck)
                                        .with_size(px(16.))
                                        .text_color(cx.theme().success),
                                )
                                .child(
                                    div()
                                        .text_base()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(cx.theme().success)
                                        .child("Training complete — 8/8"),
                                ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    "Every lesson landed in the sandbox ledger. \
                                     Reset to run it again, or head back to the register.",
                                ),
                        ),
                )
            })
            .child(
                h_flex()
                    .items_start()
                    .gap_3()
                    .flex_wrap()
                    .child(self.render_lesson_column(cx, current))
                    .child(self.render_cart_column(cx)),
            )
    }

    /// n/8 with a fraction bar, then the full checklist with checkmarks.
    fn render_progress(
        &self,
        cx: &Context<Self>,
        completed: usize,
        all_done: bool,
    ) -> impl IntoElement {
        let fraction = completed as f32 / LESSONS.len() as f32;
        v_flex()
            .gap_1p5()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(format!(
                                "PROGRESS — {} OF {} LESSONS",
                                completed,
                                LESSONS.len()
                            ))),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_features(crate::theme::tabular())
                            .text_color(if all_done {
                                cx.theme().success
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(SharedString::from(format!("{completed}/{}", LESSONS.len()))),
                    ),
            )
            .child(
                div()
                    .h(px(6.))
                    .w_full()
                    .rounded(px(3.))
                    .bg(cx.theme().sidebar_accent)
                    .child(
                        h_flex()
                            .h_full()
                            .w_full()
                            .child(div().h_full().flex_grow(fraction).rounded(px(3.)).bg(
                                if all_done {
                                    cx.theme().success
                                } else {
                                    cx.theme().accent
                                },
                            ))
                            .child(div().h_full().flex_grow(1.0 - fraction)),
                    ),
            )
            .child(h_flex().gap_2().flex_wrap().children(LESSONS.iter().map(
                |(number, title, _)| {
                    let done = self
                        .session
                        .as_ref()
                        .is_some_and(|s| s.completed.contains(number));
                    let is_current = *number == self.current_lesson();
                    h_flex()
                        .id(SharedString::from(format!("lesson-chip-{number}")))
                        .gap_1p5()
                        .items_center()
                        .px_2()
                        .py_1()
                        .rounded(px(999.))
                        .border_1()
                        .border_color(if done {
                            cx.theme().success
                        } else if is_current {
                            crate::theme::accent_dim()
                        } else {
                            cx.theme().border
                        })
                        .child(if done {
                            Icon::new(IconName::CircleCheck)
                                .with_size(px(12.))
                                .text_color(cx.theme().success)
                                .into_any_element()
                        } else {
                            div()
                                .text_size(px(10.5))
                                .font_features(crate::theme::tabular())
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(number.to_string()))
                                .into_any_element()
                        })
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(if done {
                                    cx.theme().muted_foreground
                                } else if is_current {
                                    cx.theme().accent
                                } else {
                                    cx.theme().foreground
                                })
                                .child(SharedString::from(*title)),
                        )
                },
            )))
    }

    /// The current lesson's instruction and its interactive control.
    fn render_lesson_column(&self, cx: &Context<Self>, current: u8) -> impl IntoElement {
        let lesson = LESSONS
            .iter()
            .find(|(number, _, _)| *number == current)
            .copied();
        let mut column = v_flex().flex_grow(1.4).min_w(px(360.)).gap_3();
        match (self.session.as_ref(), lesson) {
            (Some(_), Some((number, title, instruction))) => {
                column = column.child(
                    v_flex()
                        .gap_2()
                        .p(px(14.))
                        .rounded(crate::theme::CARD_RADIUS)
                        .bg(cx.theme().secondary)
                        .border_1()
                        .border_color(crate::theme::accent_dim())
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .font_features(crate::theme::tabular())
                                        .text_color(cx.theme().accent)
                                        .child(SharedString::from(format!(
                                            "LESSON {number} OF {}",
                                            LESSONS.len()
                                        ))),
                                )
                                .child(
                                    div()
                                        .text_base()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(SharedString::from(title)),
                                ),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(instruction)),
                        )
                        .child(self.render_lesson_control(number, cx)),
                );
            }
            (Some(_), None) => {
                // All lessons complete; the completion banner carries it.
            }
            (None, _) => {}
        }
        column
    }

    /// The current lesson's control, inline on the lesson card.
    fn render_lesson_control(&self, lesson: u8, cx: &Context<Self>) -> impl IntoElement {
        match lesson {
            1 => h_flex()
                .gap_2()
                .flex_wrap()
                .child(
                    div()
                        .w(px(170.))
                        .child(Input::new(&self.float).id("training-float")),
                )
                .child(
                    Button::new("training-quick-float")
                        .secondary()
                        .label(SharedString::from(fmt_money(
                            LESSON_FLOAT_MINOR,
                            self.session.as_ref().map(|s| s.cur).unwrap_or(CUR_FALLBACK),
                        )))
                        .on_click(cx.listener(|this, _, window, cx| {
                            let cur = this.session.as_ref().map(|s| s.cur).unwrap_or(CUR_FALLBACK);
                            this.float.update(cx, |input, cx| {
                                input.set_value(LESSON_FLOAT_MINOR.to_string(), window, cx)
                            });
                            let _ = cur;
                            this.open_shift_from_input(window, cx);
                        })),
                )
                .child(
                    Button::new("training-open-shift")
                        .primary()
                        .label("Open shift")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_shift_from_input(window, cx);
                        })),
                )
                .into_any_element(),
            2 => div()
                .w(px(280.))
                .child(Input::new(&self.scan).id("training-scan"))
                .into_any_element(),
            4 => div()
                .w(px(220.))
                .child(Select::new(&self.customer_select))
                .into_any_element(),
            5 => h_flex()
                .gap_2()
                .child(
                    div()
                        .w(px(90.))
                        .child(Input::new(&self.discount).id("training-discount")),
                )
                .child(
                    Button::new("training-apply-discount")
                        .secondary()
                        .label("Apply %")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.apply_order_percent(cx);
                        })),
                )
                .when(
                    self.session
                        .as_ref()
                        .is_some_and(|s| s.cart.order_discount.is_some()),
                    |row| {
                        row.child(
                            Button::new("training-clear-discount")
                                .ghost()
                                .label("Clear")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_order_percent(window, cx);
                                })),
                        )
                    },
                )
                .into_any_element(),
            7 => h_flex()
                .gap_2()
                .child(
                    Button::new("training-show-receipt")
                        .secondary()
                        .label("Show receipt")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.show_receipt_again(window, cx);
                        })),
                )
                .into_any_element(),
            8 => h_flex()
                .gap_2()
                .child(
                    Button::new("training-close-shift")
                        .danger()
                        .label("Close shift…")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_close_dialog(window, cx);
                        })),
                )
                .into_any_element(),
            // Lessons 3, 6 run on the cart panel's controls.
            _ => div().into_any_element(),
        }
    }

    /// The cart panel — the register's right column, wired to the sandbox.
    fn render_cart_column(&self, cx: &Context<Self>) -> impl IntoElement {
        let Some(session) = &self.session else {
            return div().into_any_element();
        };
        let totals = cart::compute_totals(&session.cart, &session.tax_rates, session.cur);
        let customer = self.attached_customer();

        v_flex()
            .flex_grow(1.)
            .min_w(px(320.))
            .max_w(px(420.))
            .min_h_0()
            .overflow_hidden()
            .gap(px(10.))
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
                    .child(SharedString::from(format!(
                        "SANDBOX SALE — {} ITEMS",
                        session.cart.lines.len()
                    ))),
            )
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
            .child(self.render_totals(cx, &totals))
            .child(
                Button::new("training-pay")
                    .primary()
                    .large()
                    .w_full()
                    .label(SharedString::from(
                        match session.repo.active_shift().ok().flatten() {
                            Some(_) => {
                                format!("Pay — {}", fmt_money(totals.total_minor, session.cur))
                            }
                            None => "Open a shift to sell".to_string(),
                        },
                    ))
                    .disabled(session.cart.lines.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_pay(window, cx);
                    })),
            )
            .into_any_element()
    }

    fn render_cart_lines(&self, cx: &Context<Self>) -> impl IntoElement {
        let Some(session) = &self.session else {
            return div().into_any_element();
        };
        if session.cart.lines.is_empty() {
            return v_flex()
                .flex_none()
                .items_center()
                .justify_center()
                .gap_2()
                .py_6()
                .child(
                    Icon::new(IconName::Scan)
                        .with_size(px(26.))
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Sandbox cart is empty. Scan a SKU to begin."),
                )
                .into_any_element();
        }
        let mut list = v_flex().flex_none().gap_2();
        for line in &session.cart.lines {
            let gross_minor = (line.unit_minor as f64 * line.qty).round() as i64;
            list = list.child(
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
                                        fmt_money(line.unit_minor, session.cur),
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
                                    .font_features(crate::theme::tabular())
                                    .child(SharedString::from(fmt_money(gross_minor, session.cur))),
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
                    ),
            );
        }
        list.into_any_element()
    }

    fn render_totals(&self, cx: &Context<Self>, totals: &CartTotals) -> impl IntoElement {
        let Some(session) = &self.session else {
            return div().into_any_element();
        };
        let cur = session.cur;
        let row = |label: &str, value: String, color: gpui_kit::Hsla| {
            h_flex()
                .justify_between()
                .text_sm()
                .child(
                    div()
                        .text_color(color)
                        .child(SharedString::from(label.to_string())),
                )
                .child(
                    div()
                        .font_features(crate::theme::tabular())
                        .text_color(color)
                        .child(SharedString::from(value)),
                )
        };
        v_flex()
            .gap_1()
            .pt_2()
            .flex_none()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(row(
                "Subtotal",
                fmt_money(totals.gross_minor, cur),
                cx.theme().muted_foreground,
            ))
            .when(totals.discount_minor > 0, |block| {
                block.child(row(
                    "Discount",
                    format!("−{}", fmt_money(totals.discount_minor, cur)),
                    cx.theme().accent,
                ))
            })
            .child(row(
                "Tax",
                fmt_money(totals.tax_minor, cur),
                cx.theme().muted_foreground,
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
                            .font_features(crate::theme::tabular())
                            .child(SharedString::from(fmt_money(totals.total_minor, cur))),
                    ),
            )
            .into_any_element()
    }
}

/// The sandbox path as the trainee sees it in the banner/launcher.
fn sandbox_display_path() -> String {
    let source = resolve_db_path();
    Path::new(&source)
        .parent()
        .map(|dir| dir.join("training.db"))
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "./data/training.db".to_string())
}
