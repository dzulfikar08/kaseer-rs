//! Product knowledge browser/editor, ported from the web companion's
//! inventory knowledge editing (`kaseer-web` v3 schema). One screen, two
//! panes: a searchable catalog list on the left, the knowledge editor for
//! the selected product on the right. Saves go through
//! [`Repo::update_knowledge`], so both apps read the same ledger.

use std::sync::{Arc, Mutex, MutexGuard};

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::button::{Button, ButtonVariants as _, Toggle};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px,
};
use kaseer_core::repos::{KnowledgePatch, Product, Repo};

/// Poison-tolerant lock: a panic in one screen method must not wedge the
/// screen's data access forever.
fn repo_guard(repo: &Mutex<Repo>) -> MutexGuard<'_, Repo> {
    repo.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// True when the current editor inputs differ from the loaded product.
fn editor_matches(
    product: &Product,
    description: &str,
    prep: &str,
    allergens: &str,
    routed: bool,
) -> bool {
    product.description == description
        && product.prep_notes == prep
        && product.allergens == allergens
        && product.route_kitchen == routed
}

pub struct KnowledgeScreen {
    repo: Arc<Mutex<Repo>>,
    query: Entity<InputState>,
    description: Entity<InputState>,
    prep: Entity<InputState>,
    allergens: Entity<InputState>,
    /// All non-archived products; the search box filters the list live.
    products: Vec<Product>,
    selected_id: Option<String>,
    route_kitchen: bool,
    _subscriptions: Vec<Subscription>,
}

impl KnowledgeScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let repo = open_repo();
        let products = {
            let repo = repo_guard(&repo);
            repo.products(false, None).unwrap_or_default()
        };

        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search products by name or SKU"));
        let description = cx.new(|cx| InputState::new(window, cx).placeholder("What is it?"));
        let prep = cx.new(|cx| {
            InputState::new(window, cx).placeholder("How is it prepared? (station, timing, …)")
        });
        let allergens = cx.new(|cx| {
            InputState::new(window, cx).placeholder("e.g. gluten, milk, soy — 'none' if free")
        });

        let mut screen = Self {
            repo,
            query,
            description,
            prep,
            allergens,
            products,
            selected_id: None,
            route_kitchen: false,
            _subscriptions: Vec::new(),
        };

        let query = screen.query.clone();
        screen._subscriptions.push(cx.subscribe_in(
            &query,
            window,
            |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            },
        ));

        screen
    }

    /// Products matching the live search box, name-or-SKU substring — the
    /// same filter contract as `Repo::products`.
    fn filtered(&self, cx: &Context<Self>) -> Vec<Product> {
        let q = self.query.read(cx).value().trim().to_lowercase();
        self.products
            .iter()
            .filter(|p| {
                q.is_empty()
                    || p.name.to_lowercase().contains(&q)
                    || p.sku.to_lowercase().contains(&q)
            })
            .cloned()
            .collect()
    }

    fn selected(&self) -> Option<Product> {
        let id = self.selected_id.as_deref()?;
        self.products.iter().find(|p| p.id == id).cloned()
    }

    /// Point the editor at a product; empty inputs mirror empty fields.
    fn select(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(product) = self.products.iter().find(|p| p.id == id).cloned() else {
            return;
        };
        self.selected_id = Some(product.id.clone());
        self.route_kitchen = product.route_kitchen;
        self.description.update(cx, |input, cx| {
            input.set_value(product.description.clone(), window, cx)
        });
        self.prep.update(cx, |input, cx| {
            input.set_value(product.prep_notes.clone(), window, cx)
        });
        self.allergens.update(cx, |input, cx| {
            input.set_value(product.allergens.clone(), window, cx)
        });
        cx.notify();
    }

    /// Write the editor's current values through `update_knowledge` (the
    /// same merge contract as the web companion) and refresh the list.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(product) = self.selected() else {
            return;
        };
        let patch = KnowledgePatch {
            description: Some(self.description.read(cx).value().to_string()),
            prep_notes: Some(self.prep.read(cx).value().to_string()),
            allergens: Some(self.allergens.read(cx).value().to_string()),
            route_kitchen: Some(self.route_kitchen),
        };
        let outcome = repo_guard(&self.repo)
            .update_knowledge(&product.id, &patch)
            .map(|updated| updated.prep_notes.clone());
        match outcome {
            Ok(_) => {
                if let Ok(products) = repo_guard(&self.repo).products(false, None) {
                    self.products = products;
                }
                // Convergence: push the same patch at the companion so its
                // next menu pull reads this edit instead of clobbering it
                // (fire-and-forget; see menu_sync).
                crate::menu_sync::spawn_knowledge_push(cx, product.id.clone(), patch);
                window.push_notification(
                    Notification::success(format!("Knowledge saved for {}", product.name)),
                    cx,
                );
            }
            Err(error) => {
                window.push_notification(Notification::error(format!("Save failed: {error}")), cx)
            }
        }
        cx.notify();
    }

    // UI-integration readers for the verification drone (recipe accessors).
    #[allow(dead_code)] // recipe accessors: unused until the UI-test pass
    pub fn selected_product(&self) -> Option<Product> {
        self.selected()
    }

    #[allow(dead_code)] // recipe accessors: unused until the UI-test pass
    pub fn catalog_count(&self) -> usize {
        self.products.len()
    }

    fn render_list(&self, cx: &Context<Self>) -> impl IntoElement {
        let products = self.filtered(cx);
        let selected_id = self.selected_id.clone();
        let mut list = v_flex()
            .flex_grow(1.)
            .min_h_0()
            .gap_1()
            .overflow_y_scrollbar();
        for product in &products {
            let id = product.id.clone();
            let selected = selected_id.as_deref() == Some(product.id.as_str());
            list = list.child(
                h_flex()
                    .id(SharedString::from(format!("knowledge-item-{}", product.id)))
                    .cursor_pointer()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded(px(8.))
                    .when(selected, |row| row.bg(cx.theme().sidebar_accent))
                    .hover(|style| style.bg(cx.theme().secondary_active))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select(id.clone(), window, cx);
                    }))
                    .child(
                        v_flex().min_w_0().child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(SharedString::from(product.name.clone())),
                        ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(product.sku.clone())),
                            )
                            .when(product.route_kitchen, |row| {
                                row.child(
                                    Icon::new(IconName::Flame)
                                        .with_size(px(12.))
                                        .text_color(crate::theme::accent_dim()),
                                )
                            }),
                    ),
            );
        }
        if products.is_empty() {
            list = list.child(
                div()
                    .p_4()
                    .text_center()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("No products match."),
            );
        }
        list
    }

    fn render_editor(&self, cx: &Context<Self>) -> impl IntoElement {
        match self.selected() {
            None => v_flex()
                .flex_grow(1.)
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    Icon::new(IconName::BookOpen)
                        .with_size(px(26.))
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Select a product to edit its knowledge."),
                )
                .into_any_element(),
            Some(product) => {
                let description = self.description.read(cx).value().to_string();
                let prep = self.prep.read(cx).value().to_string();
                let allergens = self.allergens.read(cx).value().to_string();
                let dirty = !editor_matches(
                    &product,
                    &description,
                    &prep,
                    &allergens,
                    self.route_kitchen,
                );

                v_flex()
                    .flex_grow(1.)
                    .min_h_0()
                    .gap_3()
                    .p(px(14.))
                    .rounded(crate::theme::CARD_RADIUS)
                    .bg(cx.theme().secondary)
                    .border_1()
                    .border_color(cx.theme().border)
                    .overflow_y_scrollbar()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(SharedString::from(product.name.clone())),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(format!(
                                        "{} — {}",
                                        product.sku, product.category
                                    ))),
                            ),
                    )
                    .child(self.field(cx, "kb-description", "Description", &self.description))
                    .child(self.field(cx, "kb-prep", "Prep notes", &self.prep))
                    .child(self.field(cx, "kb-allergens", "Allergens", &self.allergens))
                    .child(
                        Toggle::new("kb-route")
                            .label("Route to kitchen")
                            .checked(self.route_kitchen)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.route_kitchen = !this.route_kitchen;
                                cx.notify();
                            })),
                    )
                    .child(
                        h_flex().justify_end().child(
                            Button::new("kb-save")
                                .primary()
                                .label("Save")
                                .disabled(!dirty)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save(window, cx);
                                })),
                        ),
                    )
                    .into_any_element()
            }
        }
    }

    /// One labeled field: the web `.klabel` kicker over a full-width input.
    fn field(
        &self,
        cx: &Context<Self>,
        id: &'static str,
        label: &str,
        state: &Entity<InputState>,
    ) -> impl IntoElement {
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(label.to_uppercase())),
            )
            .child(Input::new(state).id(id).w_full())
    }
}

fn open_repo() -> Arc<Mutex<Repo>> {
    Arc::new(Mutex::new(
        kaseer_core::db::open_repo(&kaseer_core::db::resolve_db_path())
            .expect("failed to open the Kaseer ledger"),
    ))
}

impl Render for KnowledgeScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("knowledge-screen")
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                h_flex()
                    .size_full()
                    .items_stretch()
                    .gap(px(10.))
                    .p_4()
                    .child(
                        v_flex()
                            .flex_grow(1.)
                            .min_w_0()
                            .gap_2()
                            .child(Input::new(&self.query).id("knowledge-search"))
                            .child(self.render_list(cx)),
                    )
                    .child(self.render_editor(cx)),
            )
    }
}
