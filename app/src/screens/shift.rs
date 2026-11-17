//! Shift screen: open the drawer with a counted float, watch the live
//! session, close with a physical count. Cash expectation always comes from
//! the ledger (`cash_expected_minor` over `repo.shift_sales`); the variance
//! line is the truth this screen exists to show. Feature flow mirrors
//! kaseer-web's shift screen; density follows the native TS reference.

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
    App, AppContext as _, Context, Div, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px,
};
use kaseer_core::currency::{Currency, currency_by_code};

use crate::theme::metrics;
use kaseer_core::db::{open_repo, resolve_db_path};
use kaseer_core::money::{self};
use kaseer_core::payments::TenderMethod;
use kaseer_core::repos::Repo;
use kaseer_core::shift::{
    DenominationCount, SettledSale, Shift, ShiftTotals, cash_expected_minor, count_to_minor,
    count_variance_minor, shift_totals,
};

const CUR_FALLBACK: &Currency = &kaseer_core::currency::IDR;
const QUICK_FLOAT: &[i64] = &[50_000, 100_000, 500_000];

/// Poison-tolerant lock: a panic in one screen method must not wedge the
/// register's data access forever.
fn repo_guard(repo: &Mutex<Repo>) -> MutexGuard<'_, Repo> {
    repo.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct ShiftScreen {
    repo: Arc<Mutex<Repo>>,
    cur: &'static Currency,
    float: Entity<InputState>,
    active: Option<ActiveShiftSummary>,
    _subscriptions: Vec<Subscription>,
}

/// Everything the live panel shows, reduced once per refresh: never queried
/// during render.
struct ActiveShiftSummary {
    shift: Shift,
    totals: ShiftTotals,
    expected_cash_minor: i64,
}

impl ShiftScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let repo = open_repo(&resolve_db_path()).expect("failed to open the Kaseer database");
        let cur = Self::currency_of(&repo);
        let repo = Arc::new(Mutex::new(repo));

        let float = cx.new(|cx| InputState::new(window, cx).placeholder("0"));
        let enter_subscription = cx.subscribe_in(&float, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.try_open(window, cx);
            }
        });

        let mut screen = Self {
            repo: repo.clone(),
            cur,
            float,
            active: None,
            _subscriptions: vec![enter_subscription],
        };
        let _ = screen.reload(cx);
        screen
    }

    fn currency_of(repo: &Repo) -> &'static Currency {
        repo.settings()
            .ok()
            .and_then(|s| currency_by_code(&s.currency_code).ok())
            .unwrap_or(CUR_FALLBACK)
    }

    /// Re-read the active shift and its settled sales into screen state.
    fn reload(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let repo = repo_guard(&self.repo);
        self.active = load_summary(&repo).map_err(|e| e.to_string())?;
        drop(repo);
        cx.notify();
        Ok(())
    }

    fn try_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let parsed = parse_float(self.float.read(cx).value().as_ref(), self.cur);
        let Some(opening_float_minor) = parsed else {
            return;
        };
        let opened = repo_guard(&self.repo).open_shift(opening_float_minor, self.cur.code);
        match opened {
            Ok(_) => {
                self.float.update(cx, |state, cx| {
                    state.set_value("", window, cx);
                });
                let _ = self.reload(cx);
            }
            Err(e) => notify_error(window, cx, format!("Could not open the shift: {e}")),
        }
    }

    /// Book the physical count, archive the shift, refresh. Returns `Err`
    /// with a user-facing message; the dialog stays open on failure.
    fn close_shift(&mut self, counted_minor: i64, cx: &mut Context<Self>) -> Result<(), String> {
        let Some(summary) = &self.active else {
            return Err("No shift is open".to_string());
        };
        let shift_id = summary.shift.id.clone();
        repo_guard(&self.repo)
            .close_shift(&shift_id, counted_minor)
            .map_err(|e| e.to_string())?;
        // Cloud sync: the shift's sales are final: push them now
        // (fire-and-forget; the outbox keeps them if the cloud is down).
        crate::sync_client::spawn_background_drain(cx);
        self.reload(cx)?;
        Ok(())
    }

    // UI-integration readers, so tests assert through the view instead of
    // reaching into private state (the recipe's accessor pattern).
    #[allow(dead_code)] // recipe accessors: unused until the UI-test pass
    pub fn is_shift_open(&self) -> bool {
        self.active.is_some()
    }

    #[allow(dead_code)] // recipe accessors: unused until the UI-test pass
    pub fn expected_cash_minor(&self) -> Option<i64> {
        self.active.as_ref().map(|s| s.expected_cash_minor)
    }

    #[allow(dead_code)] // recipe accessors: unused until the UI-test pass
    pub fn sales_this_shift(&self) -> Option<usize> {
        self.active.as_ref().map(|s| s.totals.sales_count)
    }
}

fn load_summary(repo: &Repo) -> kaseer_core::repos::Result<Option<ActiveShiftSummary>> {
    let Some(shift) = repo.active_shift()? else {
        return Ok(None);
    };
    let sales: Vec<SettledSale> = repo.shift_sales(&shift.id)?;
    let totals = shift_totals(&sales);
    let expected_cash_minor = cash_expected_minor(&shift, &sales);
    Ok(Some(ActiveShiftSummary {
        shift,
        totals,
        expected_cash_minor,
    }))
}

/// Strict float entry, like the native reference: empty or malformed text
/// simply keeps the Open button disabled.
fn parse_float(text: &str, cur: &Currency) -> Option<i64> {
    if text.trim().is_empty() {
        return None;
    }
    money::parse(text, cur).ok().map(|m| m.minor)
}

fn money_str(minor: i64, cur: &Currency) -> String {
    money::format(&money::money(minor, cur.code), Some(cur)).unwrap_or_else(|_| minor.to_string())
}

fn notify_error(window: &mut Window, cx: &mut App, message: String) {
    window.push_notification(Notification::error(message), cx);
}

/// "2026-09-27T14:03:45.123Z" → ("2026-09-27", "14:03"). RFC 3339 slices are
/// ASCII, so byte-range slicing is safe once the length checks pass.
fn split_opened_at(opened_at: &str) -> (&str, &str) {
    (
        opened_at.get(..10).unwrap_or(opened_at),
        opened_at.get(11..16).unwrap_or(""),
    )
}

impl Render for ShiftScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Dialogs and notifications mount once in the shell's overlay
        // layers; this screen only renders its body.
        div()
            .id("shift-screen")
            .size_full()
            .overflow_y_scrollbar()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(if self.active.is_none() {
                self.render_open_panel(cx).into_any_element()
            } else {
                self.render_active(cx).into_any_element()
            })
    }
}

impl ShiftScreen {
    /// No shift open: a centered card with the float input, quick amounts,
    /// and the open command.
    fn render_open_panel(&self, cx: &Context<Self>) -> impl IntoElement {
        use crate::theme::touch;
        let theme = cx.theme();
        let float_valid = parse_float(self.float.read(cx).value().as_ref(), self.cur).is_some();
        let hover = theme.secondary_hover;
        let (primary, primary_hover, on_primary) =
            (theme.primary, theme.primary_hover, theme.primary_foreground);
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(560.))
                    .gap(px(16.))
                    .p(px(28.))
                    .m(px(16.))
                    .rounded(touch::RADIUS_LG)
                    .bg(theme.secondary)
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_size(px(26.))
                            .font_weight(FontWeight::BOLD)
                            .child("Open shift"),
                    )
                    .child(
                        div()
                            .text_size(touch::FONT_BODY)
                            .text_color(theme.muted_foreground)
                            .child("Count the opening float into the drawer to start selling."),
                    )
                    .child(
                        h_flex()
                            .h(px(64.))
                            .px(px(18.))
                            .gap(px(14.))
                            .items_center()
                            .rounded(touch::RADIUS)
                            .bg(theme.background)
                            .border_1()
                            .border_color(crate::theme::border_strong())
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(touch::FONT_LABEL)
                                    .text_color(crate::theme::text_dim())
                                    .child("Opening float"),
                            )
                            .child(
                                Input::new(&self.float)
                                    .id("shift-float")
                                    .appearance(false)
                                    .flex_1()
                                    .h(px(44.))
                                    .text_size(px(24.))
                                    .line_height(px(32.)),
                            ),
                    )
                    .child(div().grid().grid_cols(3).gap(px(10.)).children(QUICK_FLOAT.iter().map(|q| {
                        div()
                            .id(SharedString::from(format!("quick-float-{q}")))
                            .cursor_pointer()
                            .h(touch::TAP)
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(touch::RADIUS)
                            .bg(theme.muted)
                            .hover(move |style| style.bg(hover))
                            .active(|style| style.opacity(0.75))
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(super::money_text(money_str(*q, self.cur)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.float.update(cx, |state, cx| {
                                    state.set_value(q.to_string(), window, cx);
                                });
                            }))
                    })))
                    .child(
                        h_flex()
                            .id("open-shift")
                            .mt(px(4.))
                            .h(touch::TAP_XL)
                            .items_center()
                            .justify_center()
                            .rounded(touch::RADIUS_LG)
                            .text_size(touch::FONT_ACTION)
                            .font_weight(FontWeight::BOLD)
                            .map(|button| {
                                if float_valid {
                                    button
                                        .cursor_pointer()
                                        .bg(primary)
                                        .text_color(on_primary)
                                        .hover(move |style| style.bg(primary_hover))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.try_open(window, cx);
                                        }))
                                } else {
                                    button
                                        .border_1()
                                        .border_color(theme.border)
                                        .text_color(crate::theme::text_dim())
                                }
                            })
                            .child("Open shift"),
                    ),
            )
    }

    fn render_active(&self, cx: &Context<Self>) -> impl IntoElement {
        let Some(summary) = &self.active else {
            return div().child("Shift closed").into_any_element();
        };
        let (opened_date, opened_time) = split_opened_at(&summary.shift.opened_at);

        v_flex()
            .w_full()
            .max_w(px(940.))
            .mx_auto()
            .p(px(12.))
            .gap(px(12.))
            .child(
                // Header row: identity + manual refresh of the live totals.
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .text_size(px(24.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(SharedString::from("Shift"))
                            .child(
                                div()
                                    .text_size(crate::theme::touch::FONT_LABEL)
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(format!(
                                        "opened {opened_date} {opened_time}"
                                    ))),
                            ),
                    )
                    .child(
                        Button::new("refresh-shift")
                            .label("Refresh")
                            .icon(IconName::RotateCw)
                            .ghost()
                            .h(crate::theme::touch::TAP_SM)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let _ = this.reload(cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .flex_wrap()
                    .child(stat_cell(cx, "Opening float", money_str(summary.shift.opening_float_minor, self.cur)))
                    .child(stat_cell(cx, "Sales this shift", summary.totals.sales_count.to_string()))
                    .child(stat_cell(cx, "Revenue", money_str(summary.totals.total_minor, self.cur)))
                    .child(stat_cell(cx, "Expected cash", money_str(summary.expected_cash_minor, self.cur))),
            )
            .child(
                h_flex()
                    .items_start()
                    .gap(px(8.))
                    .flex_wrap()
                    .child(self.render_sales_summary(cx, summary))
                    .child(self.render_payment_mix(cx, summary)),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(crate::theme::touch::FONT_LABEL)
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(
                                "Closing counts the drawer against expected cash and archives the shift.",
                            )),
                    )
                    .child(
                        h_flex()
                            .id("close-shift-open")
                            .cursor_pointer()
                            .flex_none()
                            .h(px(64.))
                            .px(px(28.))
                            .gap(px(10.))
                            .items_center()
                            .rounded(crate::theme::touch::RADIUS_LG)
                            .bg(cx.theme().danger.opacity(0.16))
                            .border_1()
                            .border_color(cx.theme().danger.opacity(0.5))
                            .text_color(cx.theme().danger)
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .hover(|style| style.opacity(0.85))
                            .child("Close shift…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_close_dialog(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// Gross / discount / tax / net, the X-report core.
    fn render_sales_summary(
        &self,
        cx: &Context<Self>,
        summary: &ActiveShiftSummary,
    ) -> impl IntoElement {
        let t = &summary.totals;
        let cur = self.cur;
        let row = |label: &str, value: String, strong: bool| {
            ledger_row(
                cx,
                SharedString::from(label.to_string()),
                SharedString::from(value),
                strong,
            )
        };
        panel(cx, "Sales summary")
            .child(row("Gross", money_str(t.gross_minor, cur), false))
            // A minus sign only when something was actually taken off:
            // "\u{2212}Rp 0" reads as a negative amount.
            .child(row(
                "Discount",
                if t.discount_minor > 0 {
                    format!("\u{2212}{}", money_str(t.discount_minor, cur))
                } else {
                    money_str(0, cur)
                },
                false,
            ))
            .child(row("Tax", money_str(t.tax_minor, cur), false))
            .child(row("Net revenue", money_str(t.total_minor, cur), true))
    }

    /// Settled amounts per tender method; only non-zero methods show.
    fn render_payment_mix(
        &self,
        cx: &Context<Self>,
        summary: &ActiveShiftSummary,
    ) -> impl IntoElement {
        let t = &summary.totals;
        let mix: Vec<(TenderMethod, i64)> = vec![
            (TenderMethod::Cash, t.by_method.cash),
            (TenderMethod::Card, t.by_method.card),
            (TenderMethod::Qris, t.by_method.qris),
            (TenderMethod::Transfer, t.by_method.transfer),
            (TenderMethod::Loyalty, t.by_method.loyalty),
        ]
        .into_iter()
        .filter(|(_, v)| *v > 0)
        .collect();

        let mut body = v_flex().gap_2();
        if mix.is_empty() {
            body = body.child(
                div()
                    .text_size(metrics::FONT_BODY)
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from("No payments settled yet.")),
            );
        } else {
            for (method, value) in mix {
                body = body.child(ledger_row(
                    cx,
                    SharedString::from(method_label(method).to_string()),
                    SharedString::from(money_str(value, self.cur)),
                    false,
                ));
            }
        }
        panel(cx, "Payment mix").child(body)
    }

    fn open_close_dialog(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(summary) = &self.active else {
            return;
        };
        let expected = summary.expected_cash_minor;
        let cur = self.cur;
        let screen = cx.entity();

        // The view is built once, here; the dialog-layer builder below re-runs
        // every frame and must only re-wrap it, or the inputs reset per frame.
        let view = cx.new(|cx| CloseShiftDialog::new(expected, cur, window, cx));

        let surface = cx.theme().popover;
        window.open_dialog(cx, move |dialog, _, _| {
            let ok_view = view.clone();
            let ok_screen = screen.clone();
            dialog
                .title("Close shift: count the drawer")
                .w(px(660.))
                .bg(surface)
                .child(view.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().trigger(|button| {
                                button.label("Cancel").ghost().h(crate::theme::touch::TAP).px(px(24.))
                            }),
                        )
                        .child(
                            DialogAction::new()
                                .child(
                                    Button::new("ok")
                                        .label("Confirm close")
                                        .primary()
                                        .h(crate::theme::touch::TAP)
                                        .px(px(28.))
                                        .text_size(px(17.)),
                                ),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let counted = ok_view.read(cx).counted_minor(cx);
                    ok_screen
                        .update(cx, |screen, cx| screen.close_shift(counted, cx))
                        .inspect_err(|message| notify_error(window, cx, message.clone()))
                        .is_ok()
                })
        });
    }
}

// ── Close-shift dialog view ─────────────────────────────────────────────

/// Owns the denomination count inputs so counted total and variance recompute
/// live while typing; the dialog frame itself is built once at open time.
/// Screen mutation happens through handles captured at open time, so this
/// view only carries what it renders.
pub(crate) struct CloseShiftDialog {
    expected_minor: i64,
    cur: &'static Currency,
    counts: Vec<(i64, Entity<InputState>)>,
    _subscriptions: Vec<Subscription>,
}

impl CloseShiftDialog {
    pub(crate) fn new(
        expected_minor: i64,
        cur: &'static Currency,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Banknote set from the currency registry, largest first: the same
        // denominations the drawer physically holds.
        let mut notes: Vec<i64> = cur.cash_notes.to_vec();
        notes.sort_unstable_by(|a, b| b.cmp(a));

        let mut counts = Vec::new();
        let mut subscriptions = Vec::new();
        for minor in notes {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("0"));
            subscriptions.push(cx.subscribe_in(&input, window, |_, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
            counts.push((minor, input));
        }

        Self {
            expected_minor,
            cur,
            counts,
            _subscriptions: subscriptions,
        }
    }

    /// Piece count for one denomination; junk clamps to zero like the web
    /// screen's `Math.max(0, Math.round(Number || 0))`.
    fn count_of(state: &InputState) -> i64 {
        state.value().trim().parse::<i64>().unwrap_or(0).max(0)
    }

    pub(crate) fn counted_minor(&self, cx: &App) -> i64 {
        let counts: Vec<DenominationCount> = self
            .counts
            .iter()
            .map(|(minor, input)| DenominationCount {
                minor: *minor,
                count: Self::count_of(input.read(cx)),
            })
            .collect();
        count_to_minor(&counts)
    }

    fn render_count_row(
        &self,
        cx: &Context<Self>,
        minor: i64,
        input: &Entity<InputState>,
    ) -> impl IntoElement {
        use crate::theme::touch;
        let theme = cx.theme();
        let count = Self::count_of(input.read(cx));
        let hover = theme.secondary_hover;
        let step = |id: String, icon: IconName, delta: i64| {
            let input = input.clone();
            div()
                .id(SharedString::from(id))
                .cursor_pointer()
                .size(touch::TAP_SM)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(10.))
                .bg(theme.secondary)
                .hover(move |style| style.bg(hover))
                .active(|style| style.opacity(0.7))
                .child(Icon::new(icon).with_size(px(20.)))
                .on_click(cx.listener(move |_, _, window, cx| {
                    let next = (Self::count_of(input.read(cx)) + delta).max(0);
                    input.update(cx, |state, cx| {
                        state.set_value(if next == 0 { String::new() } else { next.to_string() }, window, cx)
                    });
                    cx.notify();
                }))
        };
        h_flex()
            .gap(px(10.))
            .items_center()
            .child(
                super::money_text(money_str(minor, self.cur))
                    .w(px(120.))
                    .flex_none()
                    .text_size(touch::FONT_BODY)
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(step(format!("count-dec-{minor}"), IconName::Minus, -1))
            .child(
                div()
                    .w(px(80.))
                    .h(touch::TAP_SM)
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(crate::theme::border_strong())
                    .child(
                        Input::new(input)
                            .id(SharedString::from(format!("shift-count-{minor}")))
                            .appearance(false)
                            .text_size(px(18.)),
                    ),
            )
            .child(step(format!("count-inc-{minor}"), IconName::Plus, 1))
            .child(
                super::money_text(money_str(minor * count, self.cur))
                    .flex_1()
                    .text_right()
                    .text_size(touch::FONT_BODY)
                    .text_color(theme.muted_foreground),
            )
    }
}

impl Render for CloseShiftDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let counted = self.counted_minor(cx);
        let variance = count_variance_minor(self.expected_minor, counted);
        let variance_color = if variance == 0 {
            cx.theme().success
        } else {
            cx.theme().danger
        };
        let variance_text = if variance == 0 {
            format!("{}, drawer balances", money_str(variance, self.cur))
        } else {
            format!(
                "{}{} {}",
                if variance > 0 { "+" } else { "" },
                money_str(variance, self.cur),
                if variance > 0 { "over" } else { "short" },
            )
        };

        v_flex()
            .gap(px(8.))
            .child(
                div()
                    .text_size(metrics::FONT_BODY)
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(format!(
                        "Expected cash in drawer: {}",
                        money_str(self.expected_minor, self.cur)
                    ))),
            )
            .children(
                self.counts
                    .iter()
                    .map(|(minor, input)| self.render_count_row(cx, *minor, input)),
            )
            .child(div().h(px(1.)).bg(cx.theme().border))
            .child(ledger_row(
                cx,
                "Counted".into(),
                SharedString::from(money_str(counted, self.cur)),
                true,
            ))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(metrics::FONT_BODY)
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from("Variance")),
                    )
                    .child(
                        super::money_text(variance_text)
                            .text_size(metrics::FONT_HEADLINE)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(variance_color),
                    ),
            )
    }
}

// ── Shared bits ─────────────────────────────────────────────────────────

/// A KPI tile in the stats row: the shared Atur tile, sized to share the
/// row with its siblings.
fn stat_cell(cx: &App, label: &str, value: String) -> Div {
    super::stat_tile(cx, label.to_string(), value)
        .flex_1()
        .min_w(px(180.))
}

/// The shared Atur panel with a section title, sized to share a row.
fn panel(cx: &App, title: &str) -> Div {
    super::panel(cx)
        .flex_1()
        .min_w(px(280.))
        .gap(px(8.))
        .child(super::section_title(cx, title.to_string()))
}

fn ledger_row(
    cx: &App,
    label: SharedString,
    value: SharedString,
    strong: bool,
) -> impl IntoElement {
    h_flex()
        .justify_between()
        .child(
            div()
                .text_size(crate::theme::touch::FONT_LABEL)
                .when(strong, |this| this.font_weight(FontWeight::MEDIUM))
                .text_color(if strong {
                    cx.theme().foreground
                } else {
                    cx.theme().muted_foreground
                })
                .child(label),
        )
        .child(
            super::money_text(value)
                .text_size(crate::theme::touch::FONT_LABEL)
                .when(strong, |this| this.font_weight(FontWeight::MEDIUM)),
        )
}

fn method_label(method: TenderMethod) -> &'static str {
    match method {
        TenderMethod::Cash => "Cash",
        TenderMethod::Card => "Card",
        TenderMethod::Qris => "QRIS",
        TenderMethod::Transfer => "Transfer",
        TenderMethod::Loyalty => "Loyalty",
    }
}
