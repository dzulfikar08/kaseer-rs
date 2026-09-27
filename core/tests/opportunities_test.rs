//! Ported from kaseer/src/core/opportunities.test.ts — weekend demand skew
//! and restock cover. Calendar is real: 2026-09-21..25 are Mon..Fri, 26..27
//! are Sat/Sun, and every expectation is an exact integer or one-decimal.

mod common;

use std::collections::BTreeMap;

use common::{line_sale, unit_line};
use kaseer_core::opportunities::{restock_suggestions, weekend_pattern, RestockSuggestion, RestockVerdict, WeekendPattern, WeekendVerdict};
use kaseer_core::receipt::Sale;

fn sale(at: &str, lines: Vec<kaseer_core::receipt::SaleLine>) -> Sale {
    line_sale(at, lines)
}

fn weekend(product_id: &str, name: &str, rate: i64) -> WeekendPattern {
    WeekendPattern {
        product_id: product_id.to_string(),
        name: name.to_string(),
        weekend_rate: rate,
        verdict: WeekendVerdict::WeekendHeavy,
    }
}

fn restock(product_id: &str, name: &str, sold: f64, on_hand: i64, cover: f64, verdict: RestockVerdict) -> RestockSuggestion {
    RestockSuggestion {
        product_id: product_id.to_string(),
        name: name.to_string(),
        sold_in_window: sold,
        on_hand,
        days_of_cover: cover,
        verdict,
    }
}

#[test]
fn weekend_pattern_flags_only_strictly_more_than_half_of_five_plus() {
    let espresso = "p-espresso";
    let rice = "p-rice";
    let croissant = "p-croissant";
    let tea = "p-tea";
    let sales = [
        sale("2026-09-21T09:00", vec![unit_line(espresso, "Espresso", 1.0)]),
        sale("2026-09-22T09:00", vec![unit_line(espresso, "Espresso", 1.0)]),
        sale("2026-09-21T10:00", vec![unit_line(rice, "Rice", 1.0)]),
        sale("2026-09-22T10:00", vec![unit_line(rice, "Rice", 1.0)]),
        sale("2026-09-23T10:00", vec![unit_line(rice, "Rice", 1.0)]),
        sale("2026-09-24T10:00", vec![unit_line(rice, "Rice", 1.0)]),
        sale("2026-09-25T10:00", vec![unit_line(rice, "Rice", 1.0)]),
        sale("2026-09-26T09:00", vec![unit_line(espresso, "Espresso", 1.0), unit_line(croissant, "Croissant", 1.0), unit_line(tea, "Tea", 1.0)]),
        sale("2026-09-26T15:00", vec![unit_line(espresso, "Espresso", 1.0), unit_line(croissant, "Croissant", 1.0), unit_line(tea, "Tea", 1.0)]),
        sale("2026-09-27T09:00", vec![unit_line(espresso, "Espresso", 1.0), unit_line(croissant, "Croissant", 1.0), unit_line(tea, "Tea", 1.0)]),
        sale("2026-09-27T15:00", vec![unit_line(espresso, "Espresso", 1.0)]),
        sale("2026-09-23T09:00", vec![unit_line(tea, "Tea", 1.0)]),
        sale("2026-09-24T09:00", vec![unit_line(tea, "Tea", 1.0)]),
        sale("2026-09-25T09:00", vec![unit_line(tea, "Tea", 1.0)]),
    ];
    // espresso: 4 of 6 weekend -> 6667 bps, heavy. rice: 0 of 5. tea: 3 of 6,
    // exactly half, not strictly more. croissant: only 3 total, under threshold.
    assert_eq!(weekend_pattern(&sales, 5), vec![weekend(espresso, "Espresso", 6667)]);
}

#[test]
fn lower_min_sales_threshold_admits_small_products_sorted_by_rate_desc() {
    let croissant = "p-croissant";
    let espresso = "p-espresso";
    let sales = [
        sale("2026-09-26T09:00", vec![unit_line(croissant, "Croissant", 1.0)]),
        sale("2026-09-26T15:00", vec![unit_line(croissant, "Croissant", 1.0)]),
        sale("2026-09-27T09:00", vec![unit_line(croissant, "Croissant", 1.0)]),
        sale("2026-09-21T09:00", vec![unit_line(espresso, "Espresso", 1.0)]),
        sale("2026-09-22T09:00", vec![unit_line(espresso, "Espresso", 1.0)]),
        sale("2026-09-26T10:00", vec![unit_line(espresso, "Espresso", 1.0)]),
        sale("2026-09-27T10:00", vec![unit_line(espresso, "Espresso", 1.0)]),
        sale("2026-09-27T16:00", vec![unit_line(espresso, "Espresso", 1.0)]),
    ];
    assert_eq!(
        weekend_pattern(&sales, 3),
        vec![weekend(croissant, "Croissant", 10_000), weekend(espresso, "Espresso", 6000)]
    );
}

#[test]
fn no_sales_no_patterns() {
    assert!(weekend_pattern(&[], 5).is_empty());
}

#[test]
fn restock_ranks_by_days_of_cover_with_boundary_verdicts() {
    let teh = "p-teh";
    let susu = "p-susu";
    let gula = "p-gula";
    let kopi = "p-kopi";
    let old = "p-old";
    let tea = "p-tea";
    let sales = [
        // Sunday 2026-09-20 sits one day outside the 7-day window ending on the 27th.
        sale("2026-09-20T23:59", vec![unit_line(old, "Retail Only", 7.0)]),
        sale("2026-09-21T07:30", vec![unit_line(teh, "Teh Tubruk", 7.0)]),
        sale("2026-09-24T09:00", vec![unit_line(susu, "Susu UHT", 1.0)]),
        sale("2026-09-25T09:00", vec![unit_line(gula, "Gula Pasir", 7.0)]),
        sale("2026-09-25T10:00", vec![unit_line("p-abon", "Abon Sapi", 3.0)]),
        sale(
            "2026-09-26T09:00",
            vec![
                unit_line(kopi, "Kopi Bubuk", 1.0),
                unit_line("p-edge7", "Edge Seven", 7.0),
                unit_line("p-out", "Habis", 5.0),
                unit_line("p-ghost", "Tak Terlacak", 3.0),
            ],
        ),
        sale("2026-09-26T11:00", vec![unit_line(tea, "Tea Tarik", 7.0), unit_line("p-edge14", "Edge Fourteen", 7.0)]),
        sale("2026-09-27T15:00", vec![unit_line(susu, "Susu UHT", 13.0)]),
    ];
    let on_hand = BTreeMap::from([
        (teh.to_string(), 3),
        (susu.to_string(), 7),
        (gula.to_string(), 20),
        (kopi.to_string(), 5),
        (tea.to_string(), 10),
        (old.to_string(), 1),
        ("p-abon".to_string(), 8),
        ("p-edge7".to_string(), 7),
        ("p-edge14".to_string(), 14),
        ("p-out".to_string(), 0),
        // p-ghost sold but untracked: no entry at all.
    ]);
    // teh: 3*70/7 = 30 tenths. susu: 7*70/14 = 35. edge7: 70 exactly -> 'soon'.
    // tea: 100 -> 'soon'. edge14: 140 exactly -> 'ok'. abon: 8*70/3 = 186.67 -> 187.
    // gula: 200. kopi: 350.
    assert_eq!(
        restock_suggestions(&sales, &on_hand, 7),
        vec![
            restock(teh, "Teh Tubruk", 7.0, 3, 3.0, RestockVerdict::ReorderNow),
            restock(susu, "Susu UHT", 14.0, 7, 3.5, RestockVerdict::ReorderNow),
            restock("p-edge7", "Edge Seven", 7.0, 7, 7.0, RestockVerdict::Soon),
            restock(tea, "Tea Tarik", 7.0, 10, 10.0, RestockVerdict::Soon),
            restock("p-edge14", "Edge Fourteen", 7.0, 14, 14.0, RestockVerdict::Ok),
            restock("p-abon", "Abon Sapi", 3.0, 8, 18.7, RestockVerdict::Ok),
            restock(gula, "Gula Pasir", 7.0, 20, 20.0, RestockVerdict::Ok),
            restock(kopi, "Kopi Bubuk", 1.0, 5, 35.0, RestockVerdict::Ok),
        ]
    );
}

#[test]
fn default_window_is_14_days_so_sales_just_past_7_days_count_again() {
    let old = "p-old";
    let teh = "p-teh";
    let sales = [
        sale("2026-09-20T23:59", vec![unit_line(old, "Retail Only", 7.0)]),
        sale("2026-09-21T07:30", vec![unit_line(teh, "Teh Tubruk", 7.0)]),
        sale("2026-09-27T15:00", vec![unit_line(teh, "Teh Tubruk", 7.0)]),
    ];
    let on_hand = BTreeMap::from([(old.to_string(), 1), (teh.to_string(), 3)]);
    // old: 1*140/7 = 20 tenths over the 14-day window; it tops the list.
    assert_eq!(
        restock_suggestions(&sales, &on_hand, 14)[0],
        restock(old, "Retail Only", 7.0, 1, 2.0, RestockVerdict::ReorderNow)
    );
    let ids: Vec<String> = restock_suggestions(&sales, &on_hand, 14).into_iter().map(|r| r.product_id).collect();
    assert!(ids.contains(&old.to_string()));
}

#[test]
fn skips_unstocked_zero_velocity_and_empty_input() {
    let susu = "p-susu";
    let gula = "p-gula";
    let old = "p-old";
    // OLD only sold outside the 7-day window -> zero velocity -> skipped,
    // despite having stock. GULA sold inside it but has zero on hand -> skipped.
    let sales = [
        sale("2026-09-20T23:59", vec![unit_line(old, "Retail Only", 7.0)]),
        sale("2026-09-24T09:00", vec![unit_line(susu, "Susu UHT", 1.0)]),
        sale("2026-09-25T09:00", vec![unit_line(gula, "Gula Pasir", 7.0)]),
        sale("2026-09-27T15:00", vec![unit_line(susu, "Susu UHT", 13.0)]),
    ];
    let on_hand = BTreeMap::from([(susu.to_string(), 7), (gula.to_string(), 0), (old.to_string(), 4)]);
    let rows = restock_suggestions(&sales, &on_hand, 7);
    let ids: Vec<String> = rows.iter().map(|r| r.product_id.clone()).collect();
    assert_eq!(ids, vec![susu.to_string()]);
    assert!(restock_suggestions(&[], &BTreeMap::from([(susu.to_string(), 5)]), 7).is_empty());
    assert!(restock_suggestions(&sales, &on_hand, 0).is_empty());
}
