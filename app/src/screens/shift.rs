//! Shift screen: open the drawer with a counted float, watch the live
//! session, close with a physical count. Cash expectation always comes from
//! the ledger (`cash_expected_minor` over `repo.shift_sales`); the variance
//! line is the truth this screen exists to show. Feature flow mirrors
//! kaseer-web's shift screen; density follows the native TS reference.

use std::sync::{Arc, Mutex, MutexGuard};

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    input::{Input, InputEvent, InputState},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Div, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, px,
};
use kaseer_core::currency::{Currency, currency_by_code};
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

/// Everything the live panel shows, reduced once per refresh — never queried
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

        let float =
            cx.new(|cx| InputState::new(window, cx).placeholder("Opening float (Rp)"));
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
    Ok(Some(ActiveShiftSummary { shift, totals, expected_cash_minor }))
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
    money::format(&money::money(minor, cur.code), Some(cur))
        .unwrap_or_else(|_| minor.to_string())
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
            .child(
                if self.active.is_none() {
                    self.render_open_panel(cx).into_any_element()
                } else {
                    self.render_active(cx).into_any_element()
                },
            )
    }
}

impl ShiftScreen {
    /// No shift open: a centered card with the float input, quick amounts,
    /// and the open command.
    fn render_open_panel(&self, cx: &Context<Self>) -> impl IntoElement {
        let float_valid = parse_float(self.float.read(cx).value().as_ref(), self.cur).is_some();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(460.))
                    .gap_4()
                    .p_8()
                    .m_6()
                    .rounded(crate::theme::CARD_RADIUS)
                    .bg(cx.theme().secondary)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(SharedString::from("Open shift")),
                    )
                    .child(
                        div().text_sm().text_color(cx.theme().muted_foreground).child(
                            SharedString::from(
                                "Count an opening float into the drawer to start selling.",
                            ),
                        ),
                    )
                    .child(Input::new(&self.float).id("shift-float").w_full())
                    .child(h_flex().gap_2().children(QUICK_FLOAT.iter().map(|q| {
                        Button::new(SharedString::from(format!("quick-float-{q}")))
                            .label(money_str(*q, self.cur))
                            .secondary()
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.float.update(cx, |state, cx| {
                                    state.set_value(q.to_string(), window, cx);
                                });
                            }))
                    })))
                    .child(
                        Button::new("open-shift")
                            .label("Open shift")
                            .primary()
                            .large()
                            .w_full()
                            .disabled(!float_valid)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.try_open(window, cx);
                            })),
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
            .p_6()
            .gap_4()
            .child(
                // Header row: identity + manual refresh of the live totals.
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(SharedString::from("Shift"))
                            .child(
                                div()
                                    .text_base()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(format!(
                                        "opened {opened_date} {opened_time}"
                                    ))),
                            ),
                    )
                    .child(
                        Button::new("refresh-shift")
                            .label("Refresh")
                            .secondary()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                let _ = this.reload(cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(self.stat_cell(cx, "Opening float", money_str(summary.shift.opening_float_minor, self.cur), None))
                    .child(self.stat_cell(cx, "Sales this shift", summary.totals.sales_count.to_string(), None))
                    .child(self.stat_cell(cx, "Revenue", money_str(summary.totals.total_minor, self.cur), None))
                    .child(self.stat_cell(cx, "Expected cash", money_str(summary.expected_cash_minor, self.cur), None)),
            )
            .child(
                h_flex()
                    .items_start()
                    .gap_3()
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
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(
                                "Closing counts the drawer against expected cash and archives the shift.",
                            )),
                    )
                    .child(
                        Button::new("close-shift-open")
                            .label("Close shift…")
                            .danger()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_close_dialog(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn stat_cell(
        &self,
        cx: &Context<Self>,
        label: &str,
        value: String,
        color: Option<gpui_kit::Hsla>,
    ) -> impl IntoElement {
        let value_color = color.unwrap_or(cx.theme().foreground);
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
                    .text_color(value_color)
                    .child(SharedString::from(value)),
            )
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
            ledger_row(cx, SharedString::from(label.to_string()), SharedString::from(value), strong)
        };
        panel(cx, "Sales summary")
            .gap_2()
            .child(row("Gross", money_str(t.gross_minor, cur), false))
            .child(row("Discount", format!("\u{2212}{}", money_str(t.discount_minor, cur)), false))
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
                    .text_sm()
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
                .title("Close shift — count the drawer")
                .w(px(560.))
                .bg(surface)
                .child(view.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .trigger(|button| button.label("Cancel").outline().small()),
                        )
                        .child(DialogAction::new().child(
                            Button::new("ok")
                                .label("Confirm close")
                                .primary()
                                .small(),
                        )),
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
struct CloseShiftDialog {
    expected_minor: i64,
    cur: &'static Currency,
    counts: Vec<(i64, Entity<InputState>)>,
    _subscriptions: Vec<Subscription>,
}

impl CloseShiftDialog {
    fn new(
        expected_minor: i64,
        cur: &'static Currency,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Banknote set from the currency registry, largest first — the same
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

        Self { expected_minor, cur, counts, _subscriptions: subscriptions }
    }

    /// Piece count for one denomination; junk clamps to zero like the web
    /// screen's `Math.max(0, Math.round(Number || 0))`.
    fn count_of(state: &InputState) -> i64 {
        state.value().trim().parse::<i64>().unwrap_or(0).max(0)
    }

    fn counted_minor(&self, cx: &App) -> i64 {
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
        let count = Self::count_of(input.read(cx));
        h_flex()
            .gap_3()
            .child(
                div()
                    .w_24()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .font_features(crate::theme::tabular())
                    .child(SharedString::from(money_str(minor, self.cur))),
            )
            .child(Input::new(input).id(SharedString::from(format!("shift-count-{minor}"))).small().w_full())
            .child(
                div()
                    .w_24()
                    .text_right()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .font_features(crate::theme::tabular())
                    .child(SharedString::from(money_str(minor * count, self.cur))),
            )
    }
}

impl Render for CloseShiftDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let counted = self.counted_minor(cx);
        let variance = count_variance_minor(self.expected_minor, counted);
        let variance_color = if variance == 0 { cx.theme().success } else { cx.theme().danger };
        let variance_text = if variance == 0 {
            format!("{} \u{2014} ok", money_str(variance, self.cur))
        } else {
            format!(
                "{}{} {}",
                if variance > 0 { "+" } else { "" },
                money_str(variance, self.cur),
                if variance > 0 { "over" } else { "short" },
            )
        };

        v_flex()
            .gap_3()
            .child(
                div().text_sm().text_color(cx.theme().muted_foreground).child(
                    SharedString::from(format!(
                        "Expected cash in drawer: {}",
                        money_str(self.expected_minor, self.cur)
                    )),
                ),
            )
            .children(
                self.counts
                    .iter()
                    .map(|(minor, input)| self.render_count_row(cx, *minor, input)),
            )
            .child(ledger_row(
                cx,
                "Counted".into(),
                SharedString::from(money_str(counted, self.cur)),
                true,
            ))
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from("Variance")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .font_features(crate::theme::tabular())
                            .text_color(variance_color)
                            .child(SharedString::from(variance_text)),
                    ),
            )
    }
}

// ── Shared bits ─────────────────────────────────────────────────────────

/// The Kaseer panel motif: surface fill, 1px border, 12px radius, and the
/// web `.panel h3` kicker for a title.
fn panel(cx: &App, title: &str) -> Div {
    v_flex()
        .flex_1()
        .min_w(px(280.))
        .gap_2()
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
                .child(SharedString::from(title.to_uppercase())),
        )
}

fn ledger_row(cx: &App, label: SharedString, value: SharedString, strong: bool) -> impl IntoElement {
    h_flex()
        .justify_between()
        .child(
            div()
                .text_sm()
                .when(strong, |this| this.font_weight(FontWeight::MEDIUM))
                .text_color(if strong { cx.theme().foreground } else { cx.theme().muted_foreground })
                .child(label),
        )
        .child(
            div()
                .text_sm()
                .when(strong, |this| this.font_weight(FontWeight::MEDIUM))
                .font_features(crate::theme::tabular())
                .child(value),
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
