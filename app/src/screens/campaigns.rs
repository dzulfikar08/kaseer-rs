//! Campaigns screen: segment count chips plus generated draft copy with
//! clipboard copy, ported 1:1 from the web `campaigns.tsx`. The heavy
//! lifting (segment classification, draft copy, bestseller ranking) lives in
//! `kaseer-core`; this screen only composes the queries and renders.

use std::sync::Mutex;

use gpui_kit::base::{StyledExt as _, h_flex, v_flex};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, button::Button, button::ButtonVariants as _,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::{
    ClipboardItem, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Window, div,
};
use kaseer_core::campaigns::{CampaignChannel, CampaignDraft, CampaignInput, TopProduct, generate_campaigns};
use kaseer_core::currency::currency_by_code;
use kaseer_core::db::{open_repo, resolve_db_path};
use kaseer_core::product_perf::best_sellers;
use kaseer_core::repos::Repo;
use kaseer_core::reports::ms_to_iso;
use kaseer_core::segments::segments;

/// Ledger bounds for "every settled sale", matching the web server's
/// ISO-range queries over RFC 3339 stamps.
const EPOCH_START: &str = "0000-01-01T00:00:00.000Z";
const EPOCH_END: &str = "9999-12-31T23:59:59.999Z";

/// Bestseller look-back for the product drafts, matching the web server.
const BESTSELLER_WINDOW_DAYS: i64 = 30;

/// The chip labels and their counts, in the web screen's order.
const SEGMENT_CHIPS: [&str; 4] = ["New", "Loyal", "Lost", "High value"];

/// Everything the panel needs after one successful load.
struct CampaignsData {
    counts: [i64; 4],
    drafts: Vec<CampaignDraft>,
}

pub struct CampaignsScreen {
    repo: Option<Mutex<Repo>>,
    data: Option<CampaignsData>,
    error: Option<String>,
}

impl CampaignsScreen {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            repo: match open_repo(&resolve_db_path()) {
                Ok(repo) => Some(Mutex::new(repo)),
                Err(e) => {
                    let mut this = Self { repo: None, data: None, error: None };
                    this.error = Some(e.to_string());
                    return this;
                }
            },
            data: None,
            error: None,
        };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        let Some(repo) = self.repo.as_ref().and_then(|r| r.lock().ok()) else {
            self.error = Some("Database unavailable".into());
            cx.notify();
            return;
        };

        let load = || -> Result<CampaignsData, String> {
            let settings = repo.settings().map_err(|e| e.to_string())?;
            let customers = repo.customers(None).map_err(|e| e.to_string())?;
            let all_sales = repo
                .sales_between(EPOCH_START, EPOCH_END)
                .map_err(|e| e.to_string())?;

            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let now_iso = ms_to_iso(now_ms);

            let seg = segments(
                &all_sales,
                &customers,
                settings.config.lost_after_days,
                &now_iso,
            );

            let recent = repo
                .sales_between(
                    &ms_to_iso(now_ms - BESTSELLER_WINDOW_DAYS * kaseer_core::reports::MS_DAY),
                    &now_iso,
                )
                .map_err(|e| e.to_string())?;
            let top_products = best_sellers(&recent, 5)
                .into_iter()
                .map(|p| TopProduct { name: p.name, qty: p.qty.round() as i64 })
                .collect::<Vec<_>>();
            let currency_symbol = currency_by_code(&settings.currency_code)
                .map(|c| c.symbol.to_string())
                .unwrap_or_default();

            let drafts = generate_campaigns(&CampaignInput {
                store_name: settings.store_name,
                new_count: seg.new.len() as i64,
                loyal_count: seg.loyal.len() as i64,
                lost_count: seg.lost.len() as i64,
                high_value_count: seg.high_value.len() as i64,
                top_products,
                currency_symbol,
            });

            Ok(CampaignsData {
                counts: [
                    seg.new.len() as i64,
                    seg.loyal.len() as i64,
                    seg.lost.len() as i64,
                    seg.high_value.len() as i64,
                ],
                drafts,
            })
        };

        match load() {
            Ok(data) => self.data = Some(data),
            Err(e) => {
                self.data = None;
                self.error = Some(e);
            }
        }
        cx.notify();
    }

    fn copy_draft(&mut self, draft: &CampaignDraft, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(draft.body.clone()));
        window.push_notification(Notification::success("Copied"), cx);
    }
}

impl Render for CampaignsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                v_flex()
                    .size_full()
                    .p_4()
                    .gap_3()
                    .overflow_y_scrollbar()
                    .child(self.render_studio(cx))
                    .when_some(self.error.clone(), |this, error| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().danger)
                                .child(SharedString::from(error)),
                        )
                    })
                    .children(self.render_drafts(cx))
                    .when(self.data.as_ref().is_some_and(|d| d.drafts.is_empty()), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(
                                    "No drafts yet. Sales and customers feed the studio — check back after the next till day.",
                                )),
                        )
                    }),
            )
    }
}

impl CampaignsScreen {
    fn render_studio(&self, cx: &Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .p_4()
            .rounded(cx.theme().radius)
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        div()
                            .text_lg()
                            .font_medium()
                            .child(SharedString::from("Campaign studio")),
                    )
                    .child(
                        Button::new("refresh-campaigns")
                            .label("Refresh")
                            .icon(IconName::RotateCw)
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    ),
            )
            .child(
                h_flex().flex_wrap().gap_2().children(self.render_chips()),
            )
    }

    fn render_chips(&self) -> Vec<impl IntoElement> {
        SEGMENT_CHIPS
            .iter()
            .enumerate()
            .map(|(ix, label)| {
                let count = self
                    .data
                    .as_ref()
                    .map(|d| d.counts[ix].to_string())
                    .unwrap_or_else(|| "…".into());
                Tag::secondary()
                    .small()
                    .child(SharedString::from(format!("{label} — {count}")))
            })
            .collect()
    }

    fn render_drafts(&self, cx: &Context<Self>) -> Vec<impl IntoElement> {
        let Some(data) = &self.data else { return vec![] };
        data.drafts
            .iter()
            .map(|draft| {
                let title = draft.title.clone();
                let body = draft.body.clone();
                let channel = draft.channel;
                let copy_id = copy_draft_id(draft);
                let copy_target = draft.clone();
                v_flex()
                    .gap_2()
                    .p_4()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                div().text_lg().font_medium().child(SharedString::from(title)),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Tag::secondary()
                                            .small()
                                            .child(SharedString::from(channel_label(channel))),
                                    )
                                    .child(
                                        Button::new(copy_id)
                                            .xsmall()
                                            .primary()
                                            .label("Copy")
                                            .icon(IconName::Copy)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.copy_draft(&copy_target, window, cx)
                                            })),
                                    ),
                            ),
                    )
                    .child(div().text_sm().child(SharedString::from(body)))
            })
            .collect()
    }
}

/// The web screen's `CHANNEL_LABELS`.
fn channel_label(channel: CampaignChannel) -> &'static str {
    match channel {
        CampaignChannel::Whatsapp => "WhatsApp",
        CampaignChannel::Instagram => "Instagram",
        CampaignChannel::Promo => "Promo",
        CampaignChannel::Loyalty => "Loyalty",
    }
}

/// Draft titles are the generator's fixed set, so the title itself is the
/// stable element identity for each copy button.
fn copy_draft_id(draft: &CampaignDraft) -> String {
    format!("copy-{}", draft.title.to_ascii_lowercase().replace(' ', "-"))
}
