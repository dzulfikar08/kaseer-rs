//! Dashboard: the owner's morning numbers. Revenue for today, yesterday,
//! and the month; best sellers, product performance, dead and slow stock,
//! and restock timing over a 30-day window. Ported 1:1 from the web
//! dashboard screen; every number is a pure `kaseer-core` aggregate over
//! settled sales read from the SQLite repo.
//!
//! State ownership: the screen owns its `Arc<Mutex<Repo>>` (rusqlite
//! `Connection` is `!Sync`) and one loaded snapshot; `render` only
//! describes the current frame from that snapshot and never touches the
//! database.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::Button;
use gpui_kit::component::scroll::{Scrollable, ScrollableElement as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Div, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window, div, px, rems,
};
use kaseer_core::analytics::{CustomerMix, DashboardStats, customer_mix, dashboard_stats};
use kaseer_core::chrono::{
    DateTime, Datelike, Duration, Local, NaiveDate, SecondsFormat, TimeZone, Utc,
};
use kaseer_core::currency::{Currency, currency_by_code};
use kaseer_core::opportunities::{RestockSuggestion, RestockVerdict, restock_suggestions};
use kaseer_core::product_perf::{
    ProductStat, StaleStock, best_sellers, dead_inventory, product_stats, slow_movers,
};
use kaseer_core::repos::{Customer, Repo};
use kaseer_core::{db, money};

/// Lookback for product performance, stale stock, and restock velocity.
/// The web service fixes this at 30 days.
const WINDOW_DAYS: i64 = 30;
const BEST_SELLER_LIMIT: usize = 5;

/// One loaded snapshot of everything the dashboard shows.
pub(crate) struct DashboardData {
    stats: DashboardStats,
    mix: CustomerMix,
    perf: Vec<ProductStat>,
    best: Vec<ProductStat>,
    slow: Vec<StaleStock>,
    dead: Vec<StaleStock>,
    restock: Vec<RestockSuggestion>,
}

pub struct DashboardScreen {
    /// Opened once in `new`; `None` means the database could not be opened.
    repo: Option<Arc<Mutex<Repo>>>,
    open_error: Option<String>,
    loading: bool,
    load_error: Option<String>,
    data: Option<DashboardData>,
    currency: Currency,
}

impl DashboardScreen {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut screen = Self {
            repo: None,
            open_error: None,
            loading: false,
            load_error: None,
            data: None,
            currency: default_currency(),
        };
        match db::open_repo(&db::resolve_db_path()) {
            Ok(repo) => {
                screen.repo = Some(Arc::new(Mutex::new(repo)));
                screen.load(cx);
            }
            Err(err) => screen.open_error = Some(err.to_string()),
        }
        screen
    }

    /// Reload every aggregate from the repo on a background thread, then
    /// swap the snapshot in on the UI thread. The previous snapshot stays
    /// visible while `loading` is true.
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        self.loading = true;
        self.load_error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move { load_dashboard(&repo) })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match loaded {
                    Ok((currency, data)) => {
                        this.currency = currency;
                        this.data = Some(data);
                    }
                    Err(err) => this.load_error = Some(err),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ── Render ──────────────────────────────────────────────────────────

    fn render_view(&self, cx: &Context<Self>) -> AnyElement {
        let root = div()
            .size_full()
            .text_sm()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground);

        if self.repo.is_none() {
            let detail = self
                .open_error
                .as_deref()
                .unwrap_or("the database could not be opened");
            root.child(
                v_flex()
                    .size_full()
                    .gap_3()
                    .p_4()
                    .child(self.render_header(cx))
                    .child(error_line(cx, &format!("Dashboard failed: {detail}"))),
            )
            .into_any_element()
        } else {
            root.child(self.render_scroll_body(cx)).into_any_element()
        }
    }
    fn render_scroll_body(&self, cx: &Context<Self>) -> Scrollable<Div> {
        v_flex()
            .size_full()
            .gap_3()
            .p_4()
            .child(self.render_header(cx))
            .children(
                self.load_error
                    .as_ref()
                    .map(|err| error_line(cx, &format!("Dashboard failed: {err}"))),
            )
            .children(self.data.as_ref().map(|data| self.render_loaded(data, cx)))
            .when(self.data.is_none() && self.load_error.is_none(), |col| {
                col.child(empty_state(cx, "Loading dashboard…"))
            })
            .overflow_y_scrollbar()
    }

    fn render_header(&self, cx: &Context<Self>) -> Div {
        h_flex()
            .justify_between()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from("Dashboard")),
            )
            .child(
                Button::new("refresh")
                    .small()
                    .label(if self.loading {
                        "Loading…"
                    } else {
                        "Refresh"
                    })
                    .disabled(self.loading)
                    .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
            )
    }

    fn render_loaded(&self, d: &DashboardData, cx: &Context<Self>) -> Div {
        v_flex()
            .gap_3()
            .child(self.render_stat_row(d, cx))
            .child(
                h_flex()
                    .items_stretch()
                    .gap_3()
                    .child(
                        self.panel(cx)
                            .flex_grow(1.5)
                            .min_w_0()
                            .child(self.section_title(cx, "Best sellers — 30 days"))
                            .child(self.render_best_table(d, cx)),
                    )
                    .child(
                        self.panel(cx)
                            .flex_1()
                            .min_w_0()
                            .child(self.section_title(cx, "Customer mix — this month"))
                            .child(self.render_customer_mix(d, cx)),
                    ),
            )
            .child(
                h_flex()
                    .items_stretch()
                    .gap_3()
                    .child(
                        self.panel(cx)
                            .flex_grow(1.4)
                            .min_w_0()
                            .child(self.section_title(cx, "Product performance — 30 days"))
                            .child(self.render_perf_table(d, cx)),
                    )
                    .child(
                        self.panel(cx)
                            .flex_1()
                            .min_w_0()
                            .child(self.section_title(cx, "Dead & slow stock"))
                            .child(self.render_stale_stock(d, cx)),
                    ),
            )
            .child(
                self.panel(cx)
                    .child(self.section_title(cx, "Restock suggestions — 30-day velocity"))
                    .child(self.render_restock_table(d, cx)),
            )
    }

    fn render_stat_row(&self, d: &DashboardData, cx: &Context<Self>) -> Div {
        h_flex()
            .flex_wrap()
            .gap_4()
            .p_3()
            .rounded(cx.theme().radius)
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(self.stat(
                cx,
                "Revenue today",
                self.money(d.stats.revenue_today_minor),
                true,
            ))
            .child(self.stat(
                cx,
                "Revenue yesterday",
                self.money(d.stats.revenue_yesterday_minor),
                true,
            ))
            .child(self.stat(
                cx,
                "Revenue this month",
                self.money(d.stats.revenue_this_month_minor),
                true,
            ))
            .child(self.stat(
                cx,
                "Transactions today",
                d.stats.tx_today.to_string(),
                false,
            ))
            .child(self.stat(
                cx,
                "Avg basket today",
                self.money(d.stats.avg_basket_today_minor),
                true,
            ))
    }

    fn render_best_table(&self, d: &DashboardData, cx: &Context<Self>) -> Div {
        if d.best.is_empty() {
            return empty_state(cx, "No sales in the last 30 days.");
        }
        let rows = d.best.iter().map(|p| {
            table_row(cx)
                .child(cell(cx, None, &p.name, false))
                .child(cell(cx, Some(5.), &qty_str(p.qty), true))
                .child(cell(cx, Some(8.), &self.money(p.revenue_minor), true))
        });
        v_flex()
            .child(
                table_row(cx)
                    .child(header_cell(cx, None, "Product"))
                    .child(header_cell(cx, Some(5.), "Qty"))
                    .child(header_cell(cx, Some(8.), "Revenue")),
            )
            .children(rows)
    }

    fn render_customer_mix(&self, d: &DashboardData, cx: &Context<Self>) -> Div {
        let mix = &d.mix;
        let buyers = mix.new_customers + mix.returning_customers;
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_4()
                    .child(self.stat(cx, "New", mix.new_customers.to_string(), false))
                    .child(self.stat(cx, "Returning", mix.returning_customers.to_string(), false)),
            )
            .when(buyers > 0, |col| {
                let new_pct = (mix.new_customers as f64 / buyers as f64 * 100.).round() as f32;
                col.child(
                    // New-buyer share in accent, the rest of the buyer base
                    // in muted; proportional flex growth instead of a
                    // fractional width so the bar always fills its panel.
                    h_flex()
                        .h_2p5()
                        .w_full()
                        .rounded(px(5.))
                        .overflow_hidden()
                        .bg(cx.theme().muted)
                        .child(div().flex_grow(new_pct).h_full().bg(cx.theme().accent))
                        .child(div().flex_grow(100. - new_pct).h_full()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(format!(
                            "{new_pct:.0}% of {buyers} buyers this month are new."
                        ))),
                )
            })
            .when(buyers == 0, |col| {
                col.child(empty_state(cx, "No identified buyers yet."))
            })
    }

    fn render_perf_table(&self, d: &DashboardData, cx: &Context<Self>) -> Div {
        if d.perf.is_empty() {
            return empty_state(cx, "Nothing sold in the last 30 days.");
        }
        let rows = d.perf.iter().map(|p| {
            table_row(cx)
                .child(cell(cx, None, &p.name, false))
                .child(cell(cx, Some(5.), &qty_str(p.qty), true))
                .child(cell(cx, Some(8.), &self.money(p.revenue_minor), true))
                .child(cell(
                    cx,
                    Some(8.),
                    &p.margin_minor
                        .map_or_else(|| "—".to_string(), |margin| self.money(margin)),
                    true,
                ))
        });
        v_flex()
            .child(
                table_row(cx)
                    .child(header_cell(cx, None, "Product"))
                    .child(header_cell(cx, Some(5.), "Qty"))
                    .child(header_cell(cx, Some(8.), "Revenue"))
                    .child(header_cell(cx, Some(8.), "Margin")),
            )
            .children(rows)
    }

    fn render_stale_stock(&self, d: &DashboardData, cx: &Context<Self>) -> Div {
        if d.dead.is_empty() && d.slow.is_empty() {
            return empty_state(cx, "Everything is moving.");
        }
        v_flex()
            .gap_2()
            .when(!d.dead.is_empty(), |col| {
                col.child(kicker(cx, "Dead — no sales in 30 days"))
                    .children(d.dead.iter().map(|r| {
                        stale_row(
                            cx,
                            &r.name,
                            &format!(
                                "{} on hand · {}",
                                r.on_hand,
                                stale_label(r.days_since_last_sale)
                            ),
                        )
                    }))
            })
            .when(!d.slow.is_empty(), |col| {
                col.child(kicker(cx, "Slow movers"))
                    .children(d.slow.iter().map(|r| {
                        stale_row(
                            cx,
                            &r.name,
                            &format!(
                                "{} on hand · last sold {}d ago",
                                r.on_hand,
                                r.days_since_last_sale.unwrap_or_default()
                            ),
                        )
                    }))
            })
    }

    fn render_restock_table(&self, d: &DashboardData, cx: &Context<Self>) -> Div {
        if d.restock.is_empty() {
            return empty_state(cx, "No restock signals — stock levels look healthy.");
        }
        let rows = d.restock.iter().map(|r| {
            table_row(cx)
                .child(cell(cx, None, &r.name, false))
                .child(cell(cx, Some(5.), &r.on_hand.to_string(), true))
                .child(cell(cx, Some(5.), &qty_str(r.sold_in_window), true))
                .child(cell(cx, Some(5.), &format!("{}d", r.days_of_cover), true))
                .child(verdict_chip(cx, r.verdict))
        });
        v_flex()
            .child(
                table_row(cx)
                    .child(header_cell(cx, None, "Product"))
                    .child(header_cell(cx, Some(5.), "On hand"))
                    .child(header_cell(cx, Some(5.), "Sold"))
                    .child(header_cell(cx, Some(5.), "Cover"))
                    .child(header_cell(cx, Some(9.), "Verdict")),
            )
            .children(rows)
    }

    // ── Shared building blocks ──────────────────────────────────────────

    /// A bordered surface, matching the web screen's `.panel`.
    fn panel(&self, cx: &Context<Self>) -> Div {
        v_flex()
            .gap_2()
            .p_3()
            .rounded(cx.theme().radius)
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
    }

    fn section_title(&self, cx: &Context<Self>, title: &str) -> Div {
        kicker(cx, title)
    }

    /// An uppercase label over a large value; money values use the mono
    /// face like the ledger line on the register.
    fn stat(&self, cx: &Context<Self>, label: &str, value: String, is_money: bool) -> Div {
        v_flex()
            .min_w(rems(8.75))
            .gap_0p5()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(label.to_uppercase())),
            )
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(is_money, |value| {
                        value.font_family(cx.theme().mono_font_family.clone())
                    })
                    .child(SharedString::from(value)),
            )
    }

    fn money(&self, minor: i64) -> String {
        format_money(minor, &self.currency)
    }
}

impl Render for DashboardScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_view(cx)
    }
}

// ── Data loading ────────────────────────────────────────────────────────────

/// Lift a repo error into the screen's String error channel.
fn soft<T>(result: kaseer_core::repos::Result<T>) -> Result<T, String> {
    result.map_err(|err| err.to_string())
}

/// Read every window the dashboard needs and aggregate. Runs on the
/// background executor; `Repo` is only touched behind the mutex.
pub(crate) fn load_dashboard(repo: &Arc<Mutex<Repo>>) -> Result<(Currency, DashboardData), String> {
    let repo = repo
        .lock()
        .map_err(|_| "database lock poisoned".to_string())?;

    let now = Utc::now();
    let now_iso = iso(now);
    let today = Local::now().date_naive();
    let month_start = today.with_day(1).unwrap_or(today);

    let today_sales = soft(repo.sales_between(&iso(local_midnight(today)), &now_iso))?;
    let yesterday_sales = soft(repo.sales_between(
        &iso(local_midnight(today - Duration::days(1))),
        &iso(local_midnight(today) - Duration::milliseconds(1)),
    ))?;
    let month_sales = soft(repo.sales_between(&iso(local_midnight(month_start)), &now_iso))?;
    let window_sales = soft(repo.sales_between(&iso(now - Duration::days(WINDOW_DAYS)), &now_iso))?;

    let products = soft(repo.products(false, None))?;
    let customers: Vec<Customer> = soft(repo.customers(None))?;
    let settings = soft(repo.settings())?;
    let currency = currency_by_code(&settings.currency_code)
        .map_err(|err| err.to_string())?
        .clone();

    let mut on_hand = BTreeMap::new();
    for product in &products {
        let qty = soft(repo.stock_on_hand(&product.id))?;
        on_hand.insert(product.id.clone(), qty);
    }

    // Dead inventory names come from the sale-line snapshot when a row ever
    // sold, and fall back to the product id otherwise; the web service
    // patches in the catalog name the same way.
    let mut dead = dead_inventory(&window_sales, &on_hand, WINDOW_DAYS, &now_iso);
    let name_by_id: HashMap<&str, &str> = products
        .iter()
        .map(|product| (product.id.as_str(), product.name.as_str()))
        .collect();
    for row in &mut dead {
        if let Some(name) = name_by_id.get(row.product_id.as_str()) {
            row.name = (*name).to_string();
        }
    }

    let data = DashboardData {
        stats: dashboard_stats(&today_sales, &yesterday_sales, &month_sales),
        mix: customer_mix(&month_sales, &customers, &now_iso),
        perf: product_stats(&window_sales),
        best: best_sellers(&window_sales, BEST_SELLER_LIMIT),
        slow: slow_movers(&window_sales, &products, WINDOW_DAYS, &on_hand, &now_iso),
        dead,
        restock: restock_suggestions(&window_sales, &on_hand, WINDOW_DAYS),
    };
    Ok((currency, data))
}

// ── Presentation helpers ────────────────────────────────────────────────────

fn default_currency() -> Currency {
    currency_by_code("IDR")
        .expect("IDR is in the currency registry")
        .clone()
}

/// Canonical money display from the core formatter, e.g. "Rp 12.500".
fn format_money(minor: i64, currency: &Currency) -> String {
    money::format(&money::money(minor, currency.code), Some(currency))
        .unwrap_or_else(|_| minor.to_string())
}

/// Print a sale quantity the way the web screen prints JS numbers: whole
/// quantities stay whole ("4"), fractional ones keep their fraction ("4.5").
fn qty_str(qty: f64) -> String {
    format!("{qty}")
}

fn stale_label(days_since_last_sale: Option<i64>) -> String {
    match days_since_last_sale {
        None => "never sold".to_string(),
        Some(days) => format!("idle {days}d"),
    }
}

/// Local-midnight stamp for a civil date, as UTC ISO with millisecond
/// precision (the same convention `Repo::insert_sale` writes).
fn local_midnight(date: NaiveDate) -> DateTime<Utc> {
    let naive = date.and_hms_opt(0, 0, 0).expect("midnight is a valid time");
    Local
        .from_local_datetime(&naive)
        .single()
        .unwrap_or_else(|| Local.from_utc_datetime(&naive))
        .with_timezone(&Utc)
}

fn iso(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Small uppercase section label, matching the web `.panel h3`.
fn kicker(cx: &Context<DashboardScreen>, title: &str) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(title.to_uppercase()))
}

fn empty_state(cx: &Context<DashboardScreen>, text: &str) -> Div {
    div()
        .p_4()
        .text_center()
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(text.to_string()))
}

fn error_line(cx: &Context<DashboardScreen>, message: &str) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().danger)
        .child(SharedString::from(message.to_string()))
}

/// One grid row: flexible name column plus fixed-width right columns.
fn table_row(cx: &Context<DashboardScreen>) -> Div {
    h_flex()
        .w_full()
        .gap_2()
        .py_2()
        .border_b_1()
        .border_color(cx.theme().border)
}

fn header_cell(cx: &Context<DashboardScreen>, width: Option<f32>, label: &str) -> Div {
    let base = div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground);
    sized(base, width).child(SharedString::from(label.to_uppercase()))
}

fn cell(cx: &Context<DashboardScreen>, width: Option<f32>, text: &str, mono: bool) -> Div {
    let base = div()
        .text_sm()
        .text_color(cx.theme().foreground)
        .when(mono, |c| c.font_family(cx.theme().mono_font_family.clone()));
    sized(base, width).child(SharedString::from(text.to_string()))
}

/// `None` = flexible name column (truncates), `Some(rem)` = fixed-width
/// right-aligned numeric column.
fn sized(div: Div, width: Option<f32>) -> Div {
    match width {
        Some(rem) => div.flex_none().w(rems(rem)).text_right(),
        None => div.flex_1().min_w_0().truncate(),
    }
}

fn stale_row(cx: &Context<DashboardScreen>, name: &str, detail: &str) -> Div {
    h_flex()
        .justify_between()
        .gap_2()
        .py_1()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .truncate()
                .child(SharedString::from(name.to_string())),
        )
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(detail.to_string())),
        )
}

fn verdict_chip(cx: &Context<DashboardScreen>, verdict: RestockVerdict) -> Div {
    let (bg, fg) = if verdict == RestockVerdict::ReorderNow {
        (cx.theme().accent, cx.theme().primary_foreground)
    } else {
        (cx.theme().muted, cx.theme().muted_foreground)
    };
    div()
        .flex_none()
        .w(rems(9.))
        .px_2()
        .py_0p5()
        .rounded(px(8.))
        .bg(bg)
        .text_color(fg)
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .child(SharedString::from(verdict.as_str().to_string()))
}
