//! Inventory screen: derived stock on hand, low-stock triage, and
//! ledger-backed adjustments. On-hand is derived from stock moves (never
//! stored); an adjustment is a movement with a reason, so every discrepancy
//! stays auditable. Flow mirrors kaseer-web's inventory screen.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _, Toggle},
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::{Input, InputState},
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div, px,
};
use kaseer_core::currency::{Currency, currency_by_code};
use kaseer_core::db::{open_repo, resolve_db_path};
use kaseer_core::inventory::{StockReason, StockTracking, is_low_stock};
use kaseer_core::money;
use kaseer_core::repos::{Product, Repo};

const CUR_FALLBACK: &Currency = &kaseer_core::currency::IDR;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StockFilter {
    All,
    Low,
    Out,
}

pub struct InventoryScreen {
    repo: Arc<Mutex<Repo>>,
    cur: &'static Currency,
    filter: StockFilter,
    /// Tracked, non-archived products; loaded once, refreshed after mutations.
    products: Vec<Product>,
    /// On hand per product id, from the stock-move ledger.
    stock: HashMap<String, i64>,
}

impl InventoryScreen {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        let repo = open_repo(&resolve_db_path()).expect("failed to open the Kaseer database");
        let cur = currency_of(&repo);
        let mut screen = Self {
            repo: Arc::new(Mutex::new(repo)),
            cur,
            filter: StockFilter::All,
            products: Vec::new(),
            stock: HashMap::new(),
        };
        let (products, stock) = {
            let repo = repo_guard(&screen.repo);
            load_inventory(&repo)
        };
        screen.apply_loaded(products, stock);
        screen
    }

    /// Refill screen state and drop a filter whose bucket just emptied,
    /// like the web screen.
    fn apply_loaded(&mut self, products: Vec<Product>, stock: HashMap<String, i64>) {
        self.products = products;
        self.stock = stock;
        let lows = self.count_lows();
        let outs = self.count_outs();
        if (self.filter == StockFilter::Low && lows == 0)
            || (self.filter == StockFilter::Out && outs == 0)
        {
            self.filter = StockFilter::All;
        }
    }

    fn on_hand(&self, product: &Product) -> i64 {
        self.stock.get(&product.id).copied().unwrap_or(0)
    }

    fn is_low(&self, product: &Product) -> bool {
        is_low_stock(
            &StockTracking {
                track_stock: product.track_stock,
                low_stock_at: product.low_stock_at,
            },
            self.on_hand(product),
        )
    }

    fn count_lows(&self) -> usize {
        self.products.iter().filter(|p| self.is_low(p)).count()
    }

    fn count_outs(&self) -> usize {
        self.products
            .iter()
            .filter(|p| self.on_hand(p) <= 0)
            .count()
    }

    // UI-integration readers, so tests assert through the view instead of
    // reaching into private state (the recipe's accessor pattern).
    #[allow(dead_code)] // recipe accessors: unused until the UI-test pass
    pub fn tracked_count(&self) -> usize {
        self.products.len()
    }

    #[allow(dead_code)] // recipe accessors: unused until the UI-test pass
    pub fn stock_on_hand(&self, product_id: &str) -> Option<i64> {
        self.stock.get(product_id).copied()
    }

    #[allow(dead_code)] // recipe accessors: unused until the UI-test pass
    pub fn stock_value_minor(&self) -> i64 {
        self.products
            .iter()
            .map(|p| self.on_hand(p) * p.cost_minor)
            .sum()
    }

    fn matches_filter(&self, product: &Product, filter: StockFilter) -> bool {
        match filter {
            StockFilter::All => true,
            StockFilter::Low => self.is_low(product),
            StockFilter::Out => self.on_hand(product) <= 0,
        }
    }

    /// Book the adjustment as a stock move, refresh, report the new on hand.
    fn apply_adjust(
        &mut self,
        product_id: &str,
        delta: i64,
        reason: StockReason,
        note: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<i64, String> {
        let repo = repo_guard(&self.repo);
        repo.adjust_stock(product_id, delta, reason, note, None)
            .map_err(|e| e.to_string())?;
        let (products, stock) = load_inventory(&repo);
        let on_hand = stock.get(product_id).copied().unwrap_or(0);
        drop(repo);
        self.apply_loaded(products, stock);
        cx.notify();
        Ok(on_hand)
    }

    fn open_adjust_dialog(
        &self,
        product_id: String,
        product_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let screen = cx.entity();

        // The view is built once, here; the dialog-layer builder below re-runs
        // every frame and must only re-wrap it, or the inputs reset per frame.
        let view = cx.new(|cx| AdjustStockDialog::new(window, cx));

        let surface = cx.theme().popover;
        window.open_dialog(cx, move |dialog, _, _| {
            let ok_view = view.clone();
            let ok_screen = screen.clone();
            let ok_pid = product_id.clone();
            let ok_name = product_name.clone();
            let title = SharedString::from(format!("Adjust stock — {product_name}"));
            dialog
                .title(title)
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
                                .child(Button::new("ok").label("Apply").primary().small()),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let dialog = ok_view.read(cx);
                    let qty_text = dialog.qty_value(cx);
                    let reason = dialog.reason;
                    let note = dialog.note_value(cx);
                    if qty_text.trim().is_empty() {
                        // The web screen disables Apply on empty input; a
                        // click on our always-enabled footer just no-ops.
                        return false;
                    }
                    let parsed = qty_text.trim().parse::<i64>();
                    if parsed.is_err() || parsed == Ok(0) {
                        window.push_notification(
                            Notification::error("Enter a non-zero whole number"),
                            cx,
                        );
                        return false;
                    }
                    let delta = parsed.unwrap_or(0);
                    match ok_screen.update(cx, |screen, cx| {
                        screen.apply_adjust(&ok_pid, delta, reason, note.as_deref(), cx)
                    }) {
                        Ok(on_hand) => {
                            window.push_notification(
                                Notification::success(format!("{ok_name}: {on_hand} on hand")),
                                cx,
                            );
                            true
                        }
                        Err(message) => {
                            window.push_notification(
                                Notification::error(format!("Adjust failed: {message}")),
                                cx,
                            );
                            false
                        }
                    }
                })
        });
    }
}

fn repo_guard(repo: &Mutex<Repo>) -> MutexGuard<'_, Repo> {
    repo.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Tracked catalog plus its on-hand map, read in one pass.
fn load_inventory(repo: &Repo) -> (Vec<Product>, HashMap<String, i64>) {
    let products = repo
        .products(false, None)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.track_stock)
        .collect::<Vec<_>>();
    let stock = products
        .iter()
        .map(|p| (p.id.clone(), repo.stock_on_hand(&p.id).unwrap_or(0)))
        .collect();
    (products, stock)
}

fn currency_of(repo: &Repo) -> &'static Currency {
    repo.settings()
        .ok()
        .and_then(|s| currency_by_code(&s.currency_code).ok())
        .unwrap_or(CUR_FALLBACK)
}

fn money_str(minor: i64, cur: &Currency) -> String {
    money::format(&money::money(minor, cur.code), Some(cur)).unwrap_or_else(|_| minor.to_string())
}

impl Render for InventoryScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Dialogs and notifications mount once in the shell's overlay
        // layers; this screen only renders its body.
        div()
            .id("inventory-screen")
            .size_full()
            .overflow_y_scrollbar()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_body(cx))
    }
}

impl InventoryScreen {
    fn render_body(&self, cx: &Context<Self>) -> impl IntoElement {
        let tracked = self.products.len();
        let lows = self.count_lows();
        let outs = self.count_outs();
        let value_minor: i64 = self
            .products
            .iter()
            .map(|p| self.on_hand(p) * p.cost_minor)
            .sum();

        v_flex()
            .w_full()
            .p_6()
            .gap_4()
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(self.stat_cell(cx, "Tracked SKUs", tracked.to_string(), None))
                    .child(self.stat_cell(
                        cx,
                        "Low stock",
                        lows.to_string(),
                        (lows > 0).then_some(cx.theme().warning),
                    ))
                    .child(self.stat_cell(
                        cx,
                        "Out of stock",
                        outs.to_string(),
                        (outs > 0).then_some(cx.theme().danger),
                    ))
                    .child(self.stat_cell(
                        cx,
                        "Stock value at cost",
                        money_str(value_minor, self.cur),
                        None,
                    )),
            )
            .child(self.render_stock_table(cx))
    }

    fn stat_cell(
        &self,
        cx: &Context<Self>,
        label: &str,
        value: String,
        color: Option<gpui_kit::Hsla>,
    ) -> impl IntoElement {
        v_flex()
            .flex_1()
            .min_w(px(180.))
            .gap_1()
            .p(px(14.))
            .rounded(crate::theme::CARD_RADIUS)
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(label.to_uppercase())),
            )
            .child(
                div()
                    .text_size(px(22.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .font_features(crate::theme::tabular())
                    .text_color(color.unwrap_or(cx.theme().foreground))
                    .child(SharedString::from(value)),
            )
    }

    fn render_stock_table(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut has_rows = false;
        let mut body = v_flex();
        for product in &self.products {
            if !self.matches_filter(product, self.filter) {
                continue;
            }
            has_rows = true;
            body = body.child(self.render_product_row(cx, product));
        }
        if !has_rows {
            let noun = match self.filter {
                StockFilter::Out => "out of",
                _ => "low on",
            };
            body = body.child(
                div()
                    .p_4()
                    .text_center()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(format!(
                        "Nothing {noun} stock right now."
                    ))),
            );
        }

        v_flex()
            .flex_1()
            .gap_2()
            .p(px(14.))
            .rounded(crate::theme::CARD_RADIUS)
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().muted_foreground)
                            .child("STOCK ON HAND"),
                    )
                    .child(self.render_filter_chips(cx)),
            )
            .child(self.render_table_header(cx))
            .child(body)
    }

    fn render_filter_chips(&self, cx: &Context<Self>) -> impl IntoElement {
        let lows = self.count_lows();
        let outs = self.count_outs();
        let chip =
            |cx: &Context<Self>, id: &'static str, label: SharedString, filter: StockFilter| {
                super::chip(
                    cx,
                    id,
                    label,
                    self.filter == filter,
                    cx.listener(move |this, _, _, cx| {
                        this.filter = filter;
                        cx.notify();
                    }),
                )
            };
        h_flex()
            .gap_2()
            .child(chip(cx, "filter-all", "All".into(), StockFilter::All))
            .child(chip(
                cx,
                "filter-low",
                SharedString::from(format!("Low ({lows})")),
                StockFilter::Low,
            ))
            .child(chip(
                cx,
                "filter-out",
                SharedString::from(format!("Out ({outs})")),
                StockFilter::Out,
            ))
    }

    fn render_table_header(&self, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_3()
            .px_3()
            .pb_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(self.header_cell(cx, "Product", false))
            .child(self.header_cell(cx, "Category", false))
            .child(self.header_cell(cx, "On hand", true))
            .child(self.header_cell(cx, "Low at", true))
            .child(
                div().w_20().child(
                    div()
                        .text_size(px(11.))
                        .text_color(cx.theme().muted_foreground)
                        .child("STATUS"),
                ),
            )
            .child(div().w_20())
    }

    fn header_cell(&self, cx: &Context<Self>, text: &str, right: bool) -> impl IntoElement {
        let mut cell = div()
            .text_size(px(11.))
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(text.to_uppercase()));
        if right {
            cell = cell.w_20().text_right();
        } else {
            cell = cell.flex_1().min_w_0();
        }
        cell
    }

    fn render_product_row(&self, cx: &Context<Self>, product: &Product) -> impl IntoElement {
        let on_hand = self.on_hand(product);
        let low = self.is_low(product);
        let pid = product.id.clone();
        let (status, status_color) = if on_hand <= 0 {
            ("out", cx.theme().danger)
        } else if low {
            ("low", cx.theme().warning)
        } else {
            ("ok", cx.theme().muted_foreground)
        };

        h_flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .truncate()
                            .child(SharedString::from(product.name.clone())),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(product.sku.clone())),
                    ),
            )
            .child(
                div()
                    .w_24()
                    .text_sm()
                    .truncate()
                    .child(SharedString::from(product.category.clone())),
            )
            .child(
                div()
                    .w_20()
                    .text_right()
                    .text_sm()
                    .font_features(crate::theme::tabular())
                    .child(SharedString::from(on_hand.to_string())),
            )
            .child(
                div()
                    .w_20()
                    .text_right()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .font_features(crate::theme::tabular())
                    .child(SharedString::from(product.low_stock_at.to_string())),
            )
            .child(
                div()
                    .w_20()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(status_color)
                    .child(SharedString::from(status)),
            )
            .child(
                Button::new(SharedString::from(format!("adjust-{}", product.id)))
                    .label("Adjust")
                    .outline()
                    .small()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let name = this
                            .products
                            .iter()
                            .find(|p| p.id == pid)
                            .map(|p| p.name.clone());
                        if let Some(name) = name {
                            this.open_adjust_dialog(pid.clone(), name, window, cx);
                        }
                    })),
            )
    }
}

// ── Adjust-stock dialog view ────────────────────────────────────────────

/// Owns the qty/note inputs and the selected reason for one adjust session;
/// destroyed with the dialog, so state resets between adjustments. Screen
/// mutation happens through handles captured at open time.
struct AdjustStockDialog {
    reason: StockReason,
    qty: Entity<InputState>,
    note: Entity<InputState>,
}

impl AdjustStockDialog {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let qty = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. 24"));
        let note = cx.new(|cx| InputState::new(window, cx).placeholder("Reference (optional)"));
        Self {
            reason: StockReason::Receive,
            qty,
            note,
        }
    }

    fn qty_value(&self, cx: &App) -> String {
        self.qty.read(cx).value().to_string()
    }

    fn note_value(&self, cx: &App) -> Option<String> {
        let note = self.note.read(cx).value().trim().to_string();
        (!note.is_empty()).then_some(note)
    }

    fn set_reason(&mut self, reason: StockReason, window: &mut Window, cx: &mut Context<Self>) {
        if self.reason == reason {
            return;
        }
        self.reason = reason;
        let placeholder = match reason {
            StockReason::Receive => "e.g. 24",
            _ => "e.g. -2",
        };
        self.qty.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx);
        });
        cx.notify();
    }

    fn reason_chip(
        &self,
        cx: &Context<Self>,
        id: &'static str,
        label: &str,
        reason: StockReason,
    ) -> impl IntoElement {
        Toggle::new(id)
            .label(label)
            .small()
            .checked(self.reason == reason)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_reason(reason, window, cx);
            }))
    }
}

impl Render for AdjustStockDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .w_16()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from("Reason")),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(self.reason_chip(
                                cx,
                                "reason-receive",
                                "Receive (+)",
                                StockReason::Receive,
                            ))
                            .child(self.reason_chip(
                                cx,
                                "reason-shrinkage",
                                "Shrinkage (\u{2212})",
                                StockReason::Shrinkage,
                            ))
                            .child(self.reason_chip(
                                cx,
                                "reason-adjust",
                                "Adjustment (\u{00b1})",
                                StockReason::Adjust,
                            )),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .w_16()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from("Qty")),
                    )
                    .child(Input::new(&self.qty).id("adjust-qty").small().w_full()),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .w_16()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from("Note")),
                    )
                    .child(Input::new(&self.note).id("adjust-note").small().w_full()),
            )
    }
}
