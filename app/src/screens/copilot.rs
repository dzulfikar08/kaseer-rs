//! Copilot screen: plain-language questions answered strictly from the
//! local sales ledger, ported 1:1 from the web `copilot.tsx`. Answering is
//! synchronous local I/O, so there is no spinner state to preserve.

use std::sync::Mutex;

use gpui_kit::base::{StyledExt as _, h_flex, v_flex};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, Selectable as _, button::Button,
    button::ButtonVariants as _, input::Input, input::InputEvent, input::InputState,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div,
};
use kaseer_core::copilot::{SUPPORTED_QUESTIONS, answer};
use kaseer_core::db::{open_repo, resolve_db_path};
use kaseer_core::repos::Repo;

/// The ask-a-question column: input, supported-question chips, answer.
pub struct CopilotScreen {
    /// `None` when the database could not be opened.
    repo: Option<Mutex<Repo>>,
    question: Entity<InputState>,
    answer: Option<kaseer_core::copilot::CopilotAnswer>,
    load_error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl CopilotScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (repo, load_error) = match open_repo(&resolve_db_path()) {
            Ok(repo) => (Some(Mutex::new(repo)), None),
            Err(e) => (None, Some(e.to_string())),
        };

        let question = cx.new(|cx| {
            InputState::new(window, cx).placeholder("e.g. Why are sales down this week?")
        });

        let mut this = Self {
            repo,
            question,
            answer: None,
            load_error,
            _subscriptions: Vec::new(),
        };

        this._subscriptions.push(cx.subscribe_in(
            &this.question.clone(),
            window,
            |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.ask_from_input(window, cx);
                }
            },
        ));

        this
    }

    fn ask_from_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.question.read(cx).value().trim().to_string();
        self.ask(&text, window, cx);
    }

    /// The one call into `kaseer_core::copilot::answer`; the `now` anchor
    /// pins the "this week/month" calendar so answers line up with the
    /// Reports screen's convention.
    fn ask(&mut self, question: &str, window: &mut Window, cx: &mut Context<Self>) {
        if question.is_empty() {
            return;
        }
        let Some(repo) = self.repo.as_ref().and_then(|r| r.lock().ok()) else {
            return;
        };
        let result = (|| {
            let settings = repo.settings()?;
            answer(&repo, question, &settings, kaseer_core::chrono::Utc::now())
        })();
        drop(repo);
        match result {
            Ok(answered) => self.answer = Some(answered),
            // Like the web screen's error toast: keep any previous answer.
            Err(_) => {
                window.push_notification(
                    Notification::error("Copilot could not answer that question"),
                    cx,
                );
            }
        }
        cx.notify();
    }
}

impl Render for CopilotScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .overflow_y_scrollbar()
            .flex()
            .justify_center()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                v_flex()
                    .max_w_128()
                    .w_full()
                    .p_4()
                    .gap_3()
                    .child(self.render_ask_panel(cx))
                    .children(self.render_answer(cx))
                    .when(self.answer.is_none(), |this| {
                        this.child(
                            v_flex()
                                .gap_2()
                                .p_4()
                                .rounded(cx.theme().radius)
                                .bg(cx.theme().secondary)
                                .border_1()
                                .border_color(cx.theme().border)
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(
                                    "Pick a question or type your own. Every answer is computed from your own sales ledger — nothing else.",
                                )),
                        )
                    }),
            )
    }
}

impl CopilotScreen {
    fn render_ask_panel(&self, cx: &Context<Self>) -> impl IntoElement {
        let asked = self.question_value(cx);

        v_flex()
            .gap_3()
            .p_4()
            .rounded(cx.theme().radius)
            .bg(cx.theme().secondary)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_lg()
                    .font_medium()
                    .child(SharedString::from("Ask your ledger")),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.question)))
                    .child(
                        Button::new("ask-copilot")
                            .label("Ask")
                            .primary()
                            .disabled(asked.trim().is_empty())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.ask_from_input(window, cx)
                            })),
                    ),
            )
            .when_some(self.load_error.clone(), |panel, error| {
                panel.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(SharedString::from(error)),
                )
            })
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .children(self.render_question_chips(cx)),
            )
    }

    fn question_value(&self, cx: &Context<Self>) -> String {
        self.question.read(cx).value().to_string()
    }

    /// One chip per supported question; the chip matching the input renders
    /// selected, like the web screen's `active` class.
    fn render_question_chips(&self, cx: &Context<Self>) -> Vec<impl IntoElement> {
        let asked = self.question_value(cx);
        SUPPORTED_QUESTIONS
            .iter()
            .map(|q| {
                let chip = q.to_string();
                Button::new(SharedString::from(format!("chip-{q}")))
                    .label(*q)
                    .small()
                    .outline()
                    .selected(asked == *q)
                    .disabled(self.repo.is_none())
                    .on_click(cx.listener(move |this, _, window, cx| this.ask(&chip, window, cx)))
            })
            .collect()
    }

    fn render_answer(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let current = self.answer.as_ref()?;

        Some(
            v_flex()
                .gap_3()
                .p_4()
                .rounded(cx.theme().radius)
                .bg(cx.theme().secondary)
                .border_1()
                .border_color(cx.theme().border)
                .child(div().child(SharedString::from(current.answer.clone())))
                .when(!current.matched, |panel| {
                    panel.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(
                                "That question is outside the supported set — try one of the chips above.",
                            )),
                    )
                })
                .when(!current.citations.is_empty(), |panel| {
                    panel.child(
                        v_flex()
                            .gap_1()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .pt_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(
                                        "FIGURES THIS ANSWER STANDS ON",
                                    )),
                            )
                            .children(current.citations.iter().map(|c| {
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(format!("• {c}")))
                            })),
                    )
                }),
        )
    }
}
