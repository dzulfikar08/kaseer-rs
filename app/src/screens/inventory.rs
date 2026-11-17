//! Inventory screen: derived stock on hand, low-stock triage, and
//! ledger-backed adjustments. On-hand is derived from stock moves (never
//! stored); an adjustment is a movement with a reason, so every discrepancy
//! stays auditable. Flow mirrors kaseer-web's inventory screen, sized for a
//! touch screen: a big search bar, finger-sized filters and rows, and an
//! adjust dialog with − / + steppers.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::{Input, InputEvent, InputState},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px,
};
use kaseer_core::currency::{Currency, currency_by_code};

use crate::theme::touch;
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
    search: Entity<InputState>,
    /// Lowercased search text; rows match on name, SKU or category.
    query: String,
    _subscriptions: Vec<Subscription>,
}

impl InventoryScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let repo = open_repo(&resolve_db_path()).expect("failed to open the Kaseer database");
        let cur = currency_of(&repo);
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search product, SKU or category"));
        let subscription =
            cx.subscribe_in(&search, window, |this: &mut Self, state, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query = state.read(cx).value().trim().to_lowercase();
                    cx.notify();
                }
            });
        let mut screen = Self {
            repo: Arc::new(Mutex::new(repo)),
            cur,
            filter: StockFilter::All,
            products: Vec::new(),
            stock: HashMap::new(),
            search,
            query: String::new(),
            _subscriptions: vec![subscription],
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

    fn matches_query(&self, product: &Product) -> bool {
        self.query.is_empty()
            || product.name.to_lowercase().contains(&self.query)
            || product.sku.to_lowercase().contains(&self.query)
            || product.category.to_lowercase().contains(&self.query)
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
        let on_hand = self.stock.get(&product_id).copied().unwrap_or(0);

        // The view is built once, here; the dialog-layer builder below re-runs
        // every frame and must only re-wrap it, or the inputs reset per frame.
        let view = cx.new(|cx| AdjustStockDialog::new(on_hand, window, cx));
        // Type straight away: the quantity has focus once the dialog is up.
        let focus = view.read(cx).qty.clone();
        window.defer(cx, move |window, cx| {
            focus.update(cx, |state, cx| state.focus(window, cx));
        });

        let surface = cx.theme().popover;
        window.open_dialog(cx, move |dialog, _, _| {
            let ok_view = view.clone();
            let ok_screen = screen.clone();
            let ok_pid = product_id.clone();
            let ok_name = product_name.clone();
            let title = SharedString::from(format!("Stock: {product_name}"));
            dialog
                .title(title)
                .w(px(620.))
                .bg(surface)
                .child(view.clone())
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
                                Button::new("ok")
                                    .child(button_text("Apply"))
                                    .primary()
                                    .h(touch::TAP)
                                    .px(px(32.))
                                    .large(),
                            ),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let dialog = ok_view.read(cx);
                    let reason = dialog.mode.reason();
                    let note = dialog.note_value(cx);
                    let delta = match dialog.delta(cx) {
                        Ok(delta) => delta,
                        Err(message) => {
                            window.push_notification(Notification::error(message), cx);
                            return false;
                        }
                    };
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

/// Stock table column widths, shared by the header and every row.
const COL_CATEGORY: Pixels = px(150.);
const COL_NUMBER: Pixels = px(96.);
const COL_STATUS: Pixels = px(96.);
const COL_ACTION: Pixels = px(128.);

fn money_str(minor: i64, cur: &Currency) -> String {
    money::format(&money::money(minor, cur.code), Some(cur)).unwrap_or_else(|_| minor.to_string())
}

impl Render for InventoryScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Dialogs and notifications mount once in the window root; this
        // screen only renders its body.
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
            .p(px(16.))
            .gap(px(14.))
            .child(
                h_flex()
                    .gap(px(10.))
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
            .child(
                h_flex()
                    .gap(px(12.))
                    .items_center()
                    .flex_wrap()
                    .child(self.render_search(cx))
                    .child(self.render_filter_chips(cx)),
            )
            .child(self.render_stock_table(cx))
    }

    /// The shared Atur KPI tile, with the value tinted when it carries a
    /// state (low / out of stock).
    fn stat_cell(
        &self,
        cx: &Context<Self>,
        label: &str,
        value: String,
        color: Option<gpui_kit::Hsla>,
    ) -> impl IntoElement {
        super::panel(cx)
            .flex_1()
            .min_w(px(180.))
            .gap(px(4.))
            .child(super::section_title(cx, label.to_string()))
            .child(
                super::money_text(value)
                    .text_size(px(26.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color.unwrap_or(cx.theme().foreground)),
            )
    }

    /// The 60px search bar, shaped like the register's scan bar.
    fn render_search(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        // The input's editor renders size_full; the fixed-height frame pins it.
        h_flex()
            .flex_1()
            .min_w(px(280.))
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
                    .id("inventory-search")
                    .appearance(false)
                    .text_size(px(17.))
                    .flex_1(),
            )
    }

    fn render_stock_table(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut has_rows = false;
        let mut body = v_flex();
        for product in &self.products {
            if !self.matches_filter(product, self.filter) || !self.matches_query(product) {
                continue;
            }
            has_rows = true;
            body = body.child(self.render_product_row(cx, product));
        }
        if !has_rows {
            let message = if !self.query.is_empty() {
                "No tracked product matches that search.".to_string()
            } else if self.products.is_empty() {
                "No product tracks stock yet.".to_string()
            } else {
                let noun = match self.filter {
                    StockFilter::Out => "out of",
                    _ => "low on",
                };
                format!("Nothing {noun} stock right now.")
            };
            body = body.child(
                div()
                    .py(px(28.))
                    .text_center()
                    .text_size(touch::FONT_BODY)
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(message)),
            );
        }

        super::panel(cx)
            .flex_1()
            .gap(px(4.))
            .child(self.render_table_header(cx))
            .child(body)
    }

    fn render_filter_chips(&self, cx: &Context<Self>) -> impl IntoElement {
        let lows = self.count_lows();
        let outs = self.count_outs();
        h_flex()
            .gap(px(10.))
            .child(touch_chip(
                cx,
                "filter-all",
                "All".into(),
                self.filter == StockFilter::All,
                StockFilter::All,
            ))
            .child(touch_chip(
                cx,
                "filter-low",
                SharedString::from(format!("Low ({lows})")),
                self.filter == StockFilter::Low,
                StockFilter::Low,
            ))
            .child(touch_chip(
                cx,
                "filter-out",
                SharedString::from(format!("Out ({outs})")),
                self.filter == StockFilter::Out,
                StockFilter::Out,
            ))
    }

    fn render_table_header(&self, cx: &Context<Self>) -> impl IntoElement {
        let head = |text: &str| {
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(text.to_uppercase()))
        };
        // Same widths as `render_product_row`, or the headings drift off
        // their columns.
        h_flex()
            .gap(px(14.))
            .px(px(10.))
            .pb(px(8.))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(head("Product").flex_1().min_w_0())
            .child(head("Category").w(COL_CATEGORY))
            .child(head("On hand").w(COL_NUMBER).text_right())
            .child(head("Low at").w(COL_NUMBER).text_right())
            .child(head("Status").w(COL_STATUS).pl(px(14.)))
            .child(div().w(COL_ACTION))
    }

    fn render_product_row(&self, cx: &Context<Self>, product: &Product) -> impl IntoElement {
        let on_hand = self.on_hand(product);
        let low = self.is_low(product);
        let theme = cx.theme();
        let (status, status_color) = if on_hand <= 0 {
            ("Out", theme.danger)
        } else if low {
            ("Low", theme.warning)
        } else {
            ("OK", theme.muted_foreground)
        };
        let row_pid = product.id.clone();
        let button_pid = product.id.clone();
        let hover = theme.secondary.opacity(0.5);

        // The whole row opens the stock dialog; the button says so.
        h_flex()
            .id(SharedString::from(format!("stock-row-{}", product.id)))
            .cursor_pointer()
            .min_h(px(68.))
            .items_center()
            .gap(px(14.))
            .px(px(10.))
            .py(px(8.))
            .border_b_1()
            .border_color(theme.border)
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_adjust_for(&row_pid, window, cx);
            }))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(touch::FONT_ITEM)
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(SharedString::from(product.name.clone())),
                    )
                    .child(
                        div()
                            .font_family(crate::theme::FONT_MONO)
                            .text_size(px(13.))
                            .text_color(crate::theme::text_dim())
                            .child(SharedString::from(product.sku.clone())),
                    ),
            )
            .child(
                div()
                    .w(COL_CATEGORY)
                    .text_size(touch::FONT_LABEL)
                    .text_color(theme.muted_foreground)
                    .truncate()
                    .child(SharedString::from(product.category.clone())),
            )
            .child(
                super::money_text(on_hand.to_string())
                    .w(COL_NUMBER)
                    .text_right()
                    .text_size(px(22.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if on_hand <= 0 {
                        theme.danger
                    } else {
                        theme.foreground
                    }),
            )
            .child(
                super::money_text(product.low_stock_at.to_string())
                    .w(COL_NUMBER)
                    .text_right()
                    .text_size(touch::FONT_LABEL)
                    .text_color(theme.muted_foreground),
            )
            .child(
                // Inset so the left-aligned word does not read as a suffix
                // of the right-aligned "Low at" number beside it.
                div()
                    .w(COL_STATUS)
                    .pl(px(14.))
                    .flex()
                    .child(status_pill(status, status_color)),
            )
            .child(
                h_flex().w(COL_ACTION).justify_end().child(
                    Button::new(SharedString::from(format!("adjust-{}", product.id)))
                        .child(button_text("Adjust"))
                        .secondary()
                        .h(touch::TAP_SM)
                        .px(px(22.))
                        .large()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.open_adjust_for(&button_pid, window, cx);
                        })),
                ),
            )
    }

    fn open_adjust_for(&mut self, pid: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .products
            .iter()
            .find(|p| p.id == pid)
            .map(|p| p.name.clone());
        if let Some(name) = name {
            self.open_adjust_dialog(pid.to_string(), name, window, cx);
        }
    }
}

/// A 56px filter button: the active one is accent-filled, like the
/// register's category buttons.
fn touch_chip(
    cx: &Context<InventoryScreen>,
    id: &'static str,
    label: SharedString,
    active: bool,
    filter: StockFilter,
) -> impl IntoElement {
    let theme = cx.theme();
    let hover = theme.secondary_hover;
    h_flex()
        .id(id)
        .cursor_pointer()
        .flex_none()
        .h(touch::TAP)
        .px(px(22.))
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
        .child(label)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.filter = filter;
            cx.notify();
        }))
}

/// A status word as a tinted pill: the word in the state colour over that
/// colour at 12%.
fn status_pill(label: &'static str, color: gpui_kit::Hsla) -> impl IntoElement {
    div()
        .flex_none()
        .px(px(10.))
        .py(px(3.))
        .rounded(px(999.))
        .bg(color.opacity(0.12))
        .text_color(color)
        .text_size(px(14.))
        .font_weight(FontWeight::MEDIUM)
        .child(SharedString::from(label))
}

// ── Adjust-stock dialog view ────────────────────────────────────────────

/// What the dialog books. The cashier always types a plain count; the sign
/// and the ledger reason follow from the mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdjustMode {
    /// A delivery came in: on hand goes up by the quantity.
    Receive,
    /// Spoiled, broken or missing: on hand goes down by the quantity.
    Remove,
    /// A shelf count: on hand becomes the quantity.
    Count,
}

impl AdjustMode {
    fn reason(self) -> StockReason {
        match self {
            Self::Receive => StockReason::Receive,
            Self::Remove => StockReason::Shrinkage,
            Self::Count => StockReason::Adjust,
        }
    }

    fn qty_label(self) -> &'static str {
        match self {
            Self::Receive => "Received",
            Self::Remove => "Removed (spoiled, broken or missing)",
            Self::Count => "Counted on the shelf",
        }
    }
}

/// The signed stock move a mode and a typed quantity book, or why not.
fn adjust_delta(mode: AdjustMode, qty_text: &str, on_hand: i64) -> Result<i64, &'static str> {
    let text = qty_text.trim();
    if text.is_empty() {
        return Err("Enter a quantity");
    }
    let qty = text
        .parse::<i64>()
        .ok()
        .filter(|q| *q >= 0)
        .ok_or("Enter a whole number, 0 or more")?;
    let delta = match mode {
        AdjustMode::Receive => qty,
        AdjustMode::Remove => -qty,
        AdjustMode::Count => qty - on_hand,
    };
    if delta == 0 {
        return Err(match mode {
            AdjustMode::Count => "That is already the count on hand",
            _ => "Enter a quantity above 0",
        });
    }
    Ok(delta)
}

/// Quick amounts added to the quantity with one tap.
const QUICK_QTY: [i64; 4] = [1, 6, 12, 24];

/// Owns the qty/note inputs and the selected mode for one adjust session;
/// destroyed with the dialog, so state resets between adjustments. Screen
/// mutation happens through handles captured at open time.
struct AdjustStockDialog {
    mode: AdjustMode,
    on_hand: i64,
    qty: Entity<InputState>,
    note: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl AdjustStockDialog {
    fn new(on_hand: i64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let qty = cx.new(|cx| InputState::new(window, cx).placeholder("0"));
        let note = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Supplier, invoice or reason (optional)")
        });
        // Re-render the preview line while typing.
        let subscription = cx.subscribe_in(&qty, window, |_, _, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            mode: AdjustMode::Receive,
            on_hand,
            qty,
            note,
            _subscriptions: vec![subscription],
        }
    }

    fn qty_number(&self, cx: &App) -> i64 {
        self.qty
            .read(cx)
            .value()
            .trim()
            .parse::<i64>()
            .unwrap_or(0)
            .max(0)
    }

    fn delta(&self, cx: &App) -> Result<i64, &'static str> {
        adjust_delta(self.mode, &self.qty.read(cx).value(), self.on_hand)
    }

    fn note_value(&self, cx: &App) -> Option<String> {
        let note = self.note.read(cx).value().trim().to_string();
        (!note.is_empty()).then_some(note)
    }

    fn set_qty(&self, qty: i64, window: &mut Window, cx: &mut Context<Self>) {
        let text = if qty <= 0 {
            String::new()
        } else {
            qty.to_string()
        };
        self.qty
            .update(cx, |state, cx| state.set_value(text, window, cx));
        cx.notify();
    }

    fn mode_button(
        &self,
        cx: &Context<Self>,
        id: &'static str,
        label: &'static str,
        mode: AdjustMode,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let hover = theme.secondary_hover;
        let active = self.mode == mode;
        div()
            .id(id)
            .cursor_pointer()
            .flex_1()
            .h(touch::TAP)
            .flex()
            .items_center()
            .justify_center()
            .rounded(touch::RADIUS)
            .text_size(touch::FONT_BODY)
            .font_weight(FontWeight::MEDIUM)
            .map(|button| {
                if active {
                    button
                        .bg(theme.primary)
                        .text_color(theme.primary_foreground)
                } else {
                    button
                        .bg(theme.secondary)
                        .border_1()
                        .border_color(crate::theme::border_strong())
                        .text_color(theme.muted_foreground)
                        .hover(move |style| style.bg(hover))
                }
            })
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.mode != mode {
                    this.mode = mode;
                    // A count starts from what the shelf should hold.
                    let start = if mode == AdjustMode::Count {
                        this.on_hand.max(0)
                    } else {
                        0
                    };
                    this.set_qty(start, window, cx);
                }
            }))
    }

    fn stepper(
        &self,
        cx: &Context<Self>,
        id: &'static str,
        icon: IconName,
        delta: i64,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let hover = theme.secondary_hover;
        div()
            .id(id)
            .cursor_pointer()
            .size(touch::TAP)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(touch::RADIUS)
            .bg(theme.secondary)
            .border_1()
            .border_color(crate::theme::border_strong())
            .hover(move |style| style.bg(hover))
            .active(|style| style.opacity(0.7))
            .child(Icon::new(icon).with_size(px(22.)))
            .on_click(cx.listener(move |this, _, window, cx| {
                let next = (this.qty_number(cx) + delta).max(0);
                this.set_qty(next, window, cx);
            }))
    }

    fn quick_buttons(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let hover = theme.secondary_hover;
        div()
            .grid()
            .grid_cols(4)
            .gap(px(10.))
            .children(QUICK_QTY.iter().map(|q| {
                let q = *q;
                div()
                    .id(SharedString::from(format!("quick-qty-{q}")))
                    .cursor_pointer()
                    .h(touch::TAP_SM)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(touch::RADIUS)
                    .bg(theme.muted)
                    .hover(move |style| style.bg(hover))
                    .active(|style| style.opacity(0.75))
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(format!("+{q}")))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let next = this.qty_number(cx) + q;
                        this.set_qty(next, window, cx);
                    }))
            }))
    }
}

impl Render for AdjustStockDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let preview = match self.delta(cx) {
            Ok(delta) => format!("On hand {} \u{2192} {}", self.on_hand, self.on_hand + delta),
            Err(_) => format!("On hand now: {}", self.on_hand),
        };
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let field_bg = theme.background;
        let label = move |text: &'static str| {
            div()
                .text_size(touch::FONT_LABEL)
                .text_color(muted)
                .child(text)
        };

        v_flex()
            .gap(px(16.))
            .child(
                h_flex()
                    .gap(px(10.))
                    .child(self.mode_button(cx, "mode-receive", "Receive", AdjustMode::Receive))
                    .child(self.mode_button(cx, "mode-remove", "Remove", AdjustMode::Remove))
                    .child(self.mode_button(cx, "mode-count", "Count", AdjustMode::Count)),
            )
            .child(
                v_flex()
                    .gap(px(10.))
                    .child(label(self.mode.qty_label()))
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .items_center()
                            .child(self.stepper(cx, "qty-dec", IconName::Minus, -1))
                            .child(
                                h_flex()
                                    .flex_1()
                                    .h(touch::TAP)
                                    .px(px(16.))
                                    .items_center()
                                    .rounded(touch::RADIUS)
                                    .bg(field_bg)
                                    .border_1()
                                    .border_color(crate::theme::border_strong())
                                    .child(
                                        Input::new(&self.qty)
                                            .id("adjust-qty")
                                            .appearance(false)
                                            .h(px(44.))
                                            .text_size(px(26.))
                                            .line_height(px(34.)),
                                    ),
                            )
                            .child(self.stepper(cx, "qty-inc", IconName::Plus, 1)),
                    )
                    .when(self.mode != AdjustMode::Count, |col| {
                        col.child(self.quick_buttons(cx))
                    })
                    .child(
                        super::money_text(preview)
                            .text_size(touch::FONT_BODY)
                            .font_weight(FontWeight::MEDIUM),
                    ),
            )
            .child(
                v_flex().gap(px(8.)).child(label("Note")).child(
                    h_flex()
                        .h(touch::TAP_SM)
                        .px(px(14.))
                        .items_center()
                        .rounded(touch::RADIUS)
                        .bg(field_bg)
                        .border_1()
                        .border_color(crate::theme::border_strong())
                        .child(
                            Input::new(&self.note)
                                .id("adjust-note")
                                .appearance(false)
                                .text_size(touch::FONT_BODY),
                        ),
                ),
            )
    }
}

/// A touch-size button label (Button sizes its own label text by variant).
fn button_text(text: &'static str) -> gpui_kit::Div {
    div()
        .text_size(px(17.))
        .font_weight(FontWeight::MEDIUM)
        .child(text)
}

#[cfg(test)]
mod tests {
    use super::{AdjustMode, adjust_delta};

    #[test]
    fn receive_and_remove_sign_the_typed_count() {
        assert_eq!(adjust_delta(AdjustMode::Receive, "24", 3), Ok(24));
        assert_eq!(adjust_delta(AdjustMode::Remove, " 2 ", 3), Ok(-2));
        assert!(adjust_delta(AdjustMode::Receive, "0", 3).is_err());
        assert!(adjust_delta(AdjustMode::Remove, "-2", 3).is_err());
        assert!(adjust_delta(AdjustMode::Receive, "", 3).is_err());
    }

    #[test]
    fn a_count_books_the_difference() {
        assert_eq!(adjust_delta(AdjustMode::Count, "10", 12), Ok(-2));
        assert_eq!(adjust_delta(AdjustMode::Count, "0", 5), Ok(-5));
        assert!(adjust_delta(AdjustMode::Count, "12", 12).is_err());
    }
}
