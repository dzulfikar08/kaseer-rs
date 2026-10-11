//! Ported from kaseer/src/core/bundles.test.ts — co-purchase bundle tests.
//! Fixtures use fixed ISO stamps and exact support/confidence assertions;
//! confidence is basis points, computed against the larger single-product
//! sale count.

mod common;

use common::{line_sale, unit_line};
use std::collections::HashSet;

use kaseer_core::bundles::{co_purchase_pairs, partner_suggestions, BundlePair, PartnerSuggestion};
use kaseer_core::receipt::Sale;

const COFFEE: &str = "p-coffee";
const CROISSANT: &str = "p-croissant";
const RICE: &str = "p-rice";

fn sale(at: &str, lines: Vec<kaseer_core::receipt::SaleLine>) -> Sale {
    line_sale(at, lines)
}

fn pair(a: &str, name_a: &str, b: &str, name_b: &str, support: i64, confidence: i64) -> BundlePair {
    BundlePair {
        product_id_a: a.to_string(),
        name_a: name_a.to_string(),
        product_id_b: b.to_string(),
        name_b: name_b.to_string(),
        support,
        confidence,
    }
}

#[test]
fn support_counts_sales_confidence_in_bps_over_larger_single_count() {
    let sales = [
        sale("2026-09-21T09:00", vec![unit_line(COFFEE, "Kopi Susu", 1.0), unit_line(CROISSANT, "Croissant", 1.0)]),
        sale("2026-09-22T09:00", vec![unit_line(COFFEE, "Kopi Susu", 1.0), unit_line(CROISSANT, "Croissant", 1.0)]),
        sale("2026-09-23T09:00", vec![unit_line(COFFEE, "Kopi Susu", 1.0)]),
        sale("2026-09-24T09:00", vec![unit_line(CROISSANT, "Croissant", 1.0), unit_line(RICE, "Nasi Goreng", 1.0)]),
    ];
    // singles: coffee 3, croissant 3, rice 1.
    // coffee+croissant: support 2, conf = roundHalfUpDiv(20000, 3) = 6667.
    // croissant+rice: support 1, conf = roundHalfUpDiv(10000, 3) = 3333.
    assert_eq!(
        co_purchase_pairs(&sales, 10),
        vec![
            pair(COFFEE, "Kopi Susu", CROISSANT, "Croissant", 2, 6667),
            pair(CROISSANT, "Croissant", RICE, "Nasi Goreng", 1, 3333),
        ]
    );
}

#[test]
fn opposite_line_order_merges_into_one_canonical_pair() {
    let sales = [
        sale("2026-09-21T09:00", vec![unit_line("p-2", "Gula", 1.0), unit_line("p-1", "Kopi", 1.0)]),
        sale("2026-09-22T09:00", vec![unit_line("p-1", "Kopi", 1.0), unit_line("p-2", "Gula", 1.0)]),
    ];
    assert_eq!(co_purchase_pairs(&sales, 10), vec![pair("p-1", "Kopi", "p-2", "Gula", 2, 10_000)]);
}

#[test]
fn same_product_twice_counts_one_sale_and_never_pairs_with_itself() {
    let sales = [sale("2026-09-21T09:00", vec![unit_line(COFFEE, "Kopi", 1.0), unit_line(COFFEE, "Kopi", 1.0), unit_line("p-gula", "Gula", 1.0)])];
    // If coffee were counted per line, the larger single would be 2 and confidence would read 5000.
    assert_eq!(co_purchase_pairs(&sales, 10), vec![pair(COFFEE, "Kopi", "p-gula", "Gula", 1, 10_000)]);
}

#[test]
fn a_product_renamed_between_sales_keeps_its_first_seen_name() {
    let sales = [
        sale("2026-09-21T09:00", vec![unit_line(COFFEE, "Kopi Susu", 1.0), unit_line("p-gula", "Gula Pasir", 1.0)]),
        sale("2026-09-22T09:00", vec![unit_line(COFFEE, "Kopi Susu Gula Aren", 1.0), unit_line("p-gula", "Gula Pasir", 1.0)]),
    ];
    assert_eq!(co_purchase_pairs(&sales, 10)[0].name_a, "Kopi Susu");
}

#[test]
fn equal_support_ties_break_by_confidence_desc_then_product_id() {
    let sales = [
        sale("2026-09-21T09:00", vec![unit_line("p-a", "A", 1.0), unit_line("p-b", "B", 1.0), unit_line("p-c", "C", 1.0)]),
        sale("2026-09-22T09:00", vec![unit_line("p-a", "A", 1.0), unit_line("p-b", "B", 1.0), unit_line("p-c", "C", 1.0)]),
        sale("2026-09-23T09:00", vec![unit_line("p-c", "C", 1.0), unit_line("p-d", "D", 1.0)]),
        sale("2026-09-24T09:00", vec![unit_line("p-c", "C", 1.0), unit_line("p-d", "D", 1.0)]),
        sale("2026-09-25T09:00", vec![unit_line("p-c", "C", 1.0)]),
        sale("2026-09-26T09:00", vec![unit_line("p-d", "D", 1.0)]),
    ];
    // All four pairs sit at support 2. a+b has confidence 10000; the rest
    // share 5000 and fall back to ascending productId order.
    let keys: Vec<String> = co_purchase_pairs(&sales, 10)
        .into_iter()
        .map(|p| format!("{}+{}", p.product_id_a, p.product_id_b))
        .collect();
    assert_eq!(keys, vec!["p-a+p-b", "p-a+p-c", "p-b+p-c", "p-c+p-d"]);
}

#[test]
fn limit_trims_the_strongest_pairs_zero_yields_none() {
    let sales = [
        sale("2026-09-21T09:00", vec![unit_line(COFFEE, "Kopi Susu", 1.0), unit_line(CROISSANT, "Croissant", 1.0)]),
        sale("2026-09-22T09:00", vec![unit_line(COFFEE, "Kopi Susu", 1.0), unit_line(CROISSANT, "Croissant", 1.0)]),
        sale("2026-09-23T09:00", vec![unit_line(CROISSANT, "Croissant", 1.0), unit_line(RICE, "Nasi Goreng", 1.0)]),
    ];
    assert_eq!(
        co_purchase_pairs(&sales, 1),
        vec![
            // singles: coffee 2, croissant 3 (also bought with rice) -> conf 6667.
            pair(COFFEE, "Kopi Susu", CROISSANT, "Croissant", 2, 6667),
        ]
    );
    assert!(co_purchase_pairs(&sales, 0).is_empty());
}

#[test]
fn no_sales_no_bundles() {
    assert!(co_purchase_pairs(&[], 10).is_empty());
}

#[test]
fn partner_suggestions_skip_basket_items_weak_pairs_and_unavailable_products() {
    const TEA: &str = "p-tea";
    let pairs = vec![
        pair(COFFEE, "Kopi Susu", CROISSANT, "Croissant", 27, 6000),
        pair(RICE, "Nasi Goreng", TEA, "Es Teh", 12, 5000),
        pair(COFFEE, "Kopi Susu", TEA, "Es Teh", 4, 1000),
        pair(COFFEE, "Kopi Susu", RICE, "Nasi Goreng", 1, 100),
    ];
    let basket: HashSet<&str> = [COFFEE].into_iter().collect();
    let got = partner_suggestions(&pairs, &basket, 2, 2, |_| true);
    assert_eq!(
        got,
        vec![
            PartnerSuggestion { product_id: CROISSANT.into(), anchor_id: COFFEE.into(), anchor_name: "Kopi Susu".into(), support: 27 },
            PartnerSuggestion { product_id: TEA.into(), anchor_id: COFFEE.into(), anchor_name: "Kopi Susu".into(), support: 4 },
        ]
    );
    // Partner already in the basket is never suggested; support 1 never shows.
    let both: HashSet<&str> = [COFFEE, CROISSANT].into_iter().collect();
    let got = partner_suggestions(&pairs, &both, 2, 2, |_| true);
    assert_eq!(got.iter().map(|s| s.product_id.as_str()).collect::<Vec<_>>(), vec![TEA]);
    // Archived or unknown partners are filtered by the caller's predicate.
    assert!(partner_suggestions(&pairs, &basket, 2, 2, |id| id == RICE).is_empty());
    // Empty basket, empty history: nothing to show, nothing invented.
    assert!(partner_suggestions(&pairs, &HashSet::new(), 2, 2, |_| true).is_empty());
    assert!(partner_suggestions(&[], &basket, 2, 2, |_| true).is_empty());
}
