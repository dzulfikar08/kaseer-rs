//! Reports: sales summary, daily revenue chart, payment mix, and top
//! products over a 7/30/90-day window, plus weekly totals and the hour
//! curve. Ported 1:1 from the web reports screen; every number is a pure
//! `kaseer-core` aggregate over settled sales read from the SQLite repo.
//! The revenue chart is plain flexbox bars sized against the window's
//! maximum, so it inherits the theme and needs no chart component.
//!
//! State ownership: the screen owns its `Arc<Mutex<Repo>>` (rusqlite
//! `Connection` is `!Sync`), the selected window length, and one loaded
//! snapshot; `render` never touches the database.

use std::sync::{Arc, Mutex};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::Button;
use gpui_kit::component::scroll::{Scrollable, ScrollableElement as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Div, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window, div, px, rems,
};
use kaseer_core::chrono::{DateTime, Duration, SecondsFormat, Utc};
use kaseer_core::currency::{Currency, currency_by_code};
use kaseer_core::reports::{
    HourBucket, PaymentMix, PeriodBucket, ProductLine, SalesSummary, by_day, by_hour_local,
    by_week, payment_mix, summarize, top_products,
};
use kaseer_core::repos::Repo;
use kaseer_core::{db, money};

const DAY_OPTIONS: [i64; 3] = [7, 30, 90];
const TOP_PRODUCT_LIMIT: usize = 8;

/// Chart geometry: the tallest bar and the plot area holding it, in px.
/// Fixed pixel geometry on purpose — the plot is a measured surface, like
/// the web screen's 140/180px chart, not a rem-based control.
const BAR_MAX_HEIGHT: f32 = 140.;
const CHART_HEIGHT: f32 = 180.;

/// One loaded snapshot for the selected window.
pub(crate) struct ReportsData {
    summary: SalesSummary,
    by_day: Vec<PeriodBucket>,
    by_hour: Vec<HourBucket>,
    by_week: Vec<PeriodBucket>,
    mix: PaymentMix,
    top: Vec<ProductLine>,
}

pub struct ReportsScreen {
    /// Opened once in `new`; `None` means the database could not be opened.
    repo: Option<Arc<Mutex<Repo>>>,
    open_error: Option<String>,
    days: i64,
    loading: bool,
    load_error: Option<String>,
    data: Option<ReportsData>,
    currency: Currency,
}

impl ReportsScreen {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut screen = Self {
            repo: None,
            open_error: None,
            days: DAY_OPTIONS[0],
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

    /// Reload every aggregate for the selected window on a background
    /// thread, then swap the snapshot in on the UI thread.
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let days = self.days;
        self.loading = true;
        self.load_error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move { load_reports(&repo, days) })
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
                    .child(error_line(cx, &format!("Reports failed: {detail}"))),
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
                    .map(|err| error_line(cx, &format!("Reports failed: {err}"))),
            )
            .children(self.data.as_ref().map(|data| self.render_loaded(data, cx)))
            .when(self.data.is_none() && self.load_error.is_none(), |col| {
                col.child(empty_state(cx, "Loading reports…"))
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
                    .child(SharedString::from("Reports")),
            )
            .child(
                h_flex()
                    .gap_1()
                    .children(DAY_OPTIONS.map(|days| {
                        Button::new(SharedString::from(format!("day-{days}")))
                            .small()
                            .selected(self.days == days)
                            .label(format!("{days}d"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.days != days {
                                    this.days = days;
                                    this.load(cx);
                                }
                            }))
                    }))
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
                    ),
            )
    }

    fn render_loaded(&self, d: &ReportsData, cx: &Context<Self>) -> Div {
        v_flex()
            .gap_3()
            .child(self.render_summary(d, cx))
            .child(self.render_daily_chart(d, cx))
            .child(
                h_flex()
                    .items_stretch()
                    .gap_3()
                    .child(
                        self.panel(cx)
                            .flex_1()
                            .min_w_0()
                            .child(self.section_title(cx, "Payment mix"))
                            .child(self.render_payment_mix(d, cx)),
                    )
                    .child(
                        self.panel(cx)
                            .flex_grow(1.4)
                            .min_w_0()
                            .child(self.section_title(cx, "Top products"))
                            .child(self.render_top_products(d, cx)),
                    ),
            )
            .child(
                h_flex()
                    .items_stretch()
                    .gap_3()
                    .child(
                        self.panel(cx)
                            .flex_1()
                            .min_w_0()
                            .child(self.section_title(cx, "Weekly totals"))
                            .child(self.render_bar_chart(cx, &d.by_week, |day| day.to_string())),
                    )
                    .child(
                        self.panel(cx)
                            .flex_1()
                            .min_w_0()
                            .child(self.section_title(cx, "Hour curve"))
                            .child(self.render_hour_curve(d, cx)),
                    ),
            )
    }

    fn render_summary(&self, d: &ReportsData, cx: &Context<Self>) -> Div {
        h_flex()
            .flex_wrap()
            .gap_4()
            .p_3()
            .rounded(cx.theme().radius)
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(self.stat(cx, "Revenue", self.money(d.summary.total_minor), true))
            .child(self.stat(cx, "Transactions", d.summary.count.to_string(), false))
            .child(self.stat(
                cx,
                "Avg basket",
                self.money(d.summary.avg_basket_minor),
                true,
            ))
            .child(self.stat(cx, "Discounts", self.money(d.summary.discount_minor), true))
            .child(self.stat(cx, "Tax", self.money(d.summary.tax_minor), true))
    }

    fn render_daily_chart(&self, d: &ReportsData, cx: &Context<Self>) -> Div {
        let max_day = d.by_day.iter().map(|b| b.total_minor).max().unwrap_or(0);
        let panel = self
            .panel(cx)
            .child(self.section_title(cx, &format!("Daily revenue — last {} days", self.days)))
            .when(max_day > 0, |col| {
                col.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(format!("PEAK {}", self.money(max_day)))),
                )
            });
        panel.child(
            self.render_bar_chart(cx, &d.by_day, |day| day.get(5..).unwrap_or(day).to_string()),
        )
    }

    /// Vertical bars sized against the window's maximum, the web chart as
    /// flexbox: one column per bucket, bar height proportional to the max,
    /// zero days ghosted at low opacity, labels thinned to fit.
    fn render_bar_chart(
        &self,
        cx: &Context<Self>,
        buckets: &[PeriodBucket],
        label_of: fn(&str) -> String,
    ) -> Div {
        let max = buckets.iter().map(|b| b.total_minor).max().unwrap_or(0);
        if max <= 0 {
            return empty_state(cx, "No sales in this period.");
        }
        let accent = cx.theme().accent;
        let muted = cx.theme().muted_foreground;
        // Thin the labels so ~12 stay visible no matter the window length.
        let label_step = buckets.len().div_ceil(12).max(1);
        let last = buckets.len().saturating_sub(1);

        h_flex()
            .items_end()
            .gap_1()
            .h(px(CHART_HEIGHT))
            .children(buckets.iter().enumerate().map(|(ix, bucket)| {
                let height = ((bucket.total_minor as f64 / max as f64) * BAR_MAX_HEIGHT as f64)
                    .round()
                    .max(2.) as f32;
                let show_label = ix % label_step == 0 || ix == last;
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .justify_end()
                    .gap_1()
                    .child(div().h(px(height)).min_w(px(2.)).rounded(px(4.)).bg(
                        if bucket.total_minor > 0 {
                            accent
                        } else {
                            accent.opacity(0.2)
                        },
                    ))
                    .child(
                        div()
                            .text_center()
                            .text_size(px(10.))
                            .text_color(muted)
                            .truncate()
                            .child(SharedString::from(if show_label {
                                label_of(&bucket.day)
                            } else {
                                String::new()
                            })),
                    )
            }))
    }

    fn render_payment_mix(&self, d: &ReportsData, cx: &Context<Self>) -> Div {
        let rows = [
            ("Cash", d.mix.cash),
            ("Card", d.mix.card),
            ("QRIS", d.mix.qris),
            ("Transfer", d.mix.transfer),
            ("Loyalty", d.mix.loyalty),
        ];
        let total: i64 = rows.iter().map(|(_, minor)| minor).sum();
        if total <= 0 {
            return empty_state(cx, "No payments yet.");
        }
        h_flex().flex_wrap().gap_1p5().children(
            rows.map(|(label, minor)| mix_chip(cx, &format!("{} · {}", label, self.money(minor)))),
        )
    }

    fn render_top_products(&self, d: &ReportsData, cx: &Context<Self>) -> Div {
        if d.top.is_empty() {
            return empty_state(cx, "Nothing sold in this period.");
        }
        let rows = d.top.iter().map(|p| {
            table_row(cx)
                .child(cell(cx, None, &p.name, false))
                .child(cell(cx, Some(5.), &qty_str(p.qty), true))
                .child(cell(cx, Some(8.), &self.money(p.net_minor), true))
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

    fn render_hour_curve(&self, d: &ReportsData, cx: &Context<Self>) -> Div {
        if d.by_hour.is_empty() {
            return empty_state(cx, "No sales in this period.");
        }
        h_flex()
            .flex_wrap()
            .gap_1p5()
            .children(d.by_hour.iter().map(|h| {
                mix_chip(
                    cx,
                    &format!("{:02}h · {}", h.hour, self.money(h.total_minor)),
                )
            }))
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

impl Render for ReportsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_view(cx)
    }
}

// ── Data loading ────────────────────────────────────────────────────────────

/// Lift a repo error into the screen's String error channel.
fn soft<T>(result: kaseer_core::repos::Result<T>) -> Result<T, String> {
    result.map_err(|err| err.to_string())
}

/// Read the window's sales and aggregate. Runs on the background executor;
/// `Repo` is only touched behind the mutex.
pub(crate) fn load_reports(
    repo: &Arc<Mutex<Repo>>,
    days: i64,
) -> Result<(Currency, ReportsData), String> {
    let repo = repo
        .lock()
        .map_err(|_| "database lock poisoned".to_string())?;

    let now = Utc::now();
    let now_iso = iso(now);
    let start_iso = iso(now - Duration::days(days));
    let sales = soft(repo.sales_between(&start_iso, &now_iso))?;

    let settings = soft(repo.settings())?;
    let currency = currency_by_code(&settings.currency_code)
        .map_err(|err| err.to_string())?
        .clone();

    let data = ReportsData {
        summary: summarize(&sales),
        by_day: by_day(&sales, &start_iso, &now_iso),
        by_hour: by_hour_local(&sales),
        by_week: by_week(&sales, &start_iso, &now_iso),
        mix: payment_mix(&sales),
        top: top_products(&sales, TOP_PRODUCT_LIMIT),
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

/// Print a quantity the way the web screen prints JS numbers.
fn qty_str(qty: f64) -> String {
    format!("{qty}")
}

fn iso(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Small uppercase section label, matching the web `.panel h3`.
fn kicker(cx: &Context<ReportsScreen>, title: &str) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(title.to_uppercase()))
}

fn empty_state(cx: &Context<ReportsScreen>, text: &str) -> Div {
    div()
        .p_4()
        .text_center()
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(text.to_string()))
}

fn error_line(cx: &Context<ReportsScreen>, message: &str) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().danger)
        .child(SharedString::from(message.to_string()))
}

/// One grid row: flexible name column plus fixed-width right columns.
fn table_row(cx: &Context<ReportsScreen>) -> Div {
    h_flex()
        .w_full()
        .gap_2()
        .py_2()
        .border_b_1()
        .border_color(cx.theme().border)
}

fn header_cell(cx: &Context<ReportsScreen>, width: Option<f32>, label: &str) -> Div {
    let base = div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground);
    sized(base, width).child(SharedString::from(label.to_uppercase()))
}

fn cell(cx: &Context<ReportsScreen>, width: Option<f32>, text: &str, mono: bool) -> Div {
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

/// A quiet rounded chip for read-only metadata (tender totals, hours).
fn mix_chip(cx: &Context<ReportsScreen>, text: &str) -> Div {
    div()
        .px_2()
        .py_0p5()
        .rounded(px(8.))
        .bg(cx.theme().muted)
        .border_1()
        .border_color(cx.theme().border)
        .text_xs()
        .font_family(cx.theme().mono_font_family.clone())
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(text.to_string()))
}
