//! Ported from kaseer/src/core/product-perf.test.ts — best sellers, slow
//! movers, dead inventory, revenue and margin per product. All assertions
//! are exact minor units; cost-carrying and legacy cost-free lines are both
//! exercised.

mod common;

use std::collections::BTreeMap;

use common::{catalog_product, line, line_sale};
use kaseer_core::product_perf::{best_sellers, dead_inventory, margin_by_product, product_stats, revenue_by_product, slow_movers, ProductStat};
use kaseer_core::receipt::Sale;

const NOW: &str = "2026-09-27T12:00:00.000Z";

fn s_line(product_id: &str, name: &str, qty: f64, net_minor: i64, cost_minor: Option<i64>) -> kaseer_core::receipt::SaleLine {
    line(product_id, name, qty, net_minor, cost_minor)
}

fn sale(at: &str, lines: Vec<kaseer_core::receipt::SaleLine>) -> Sale {
    line_sale(at, lines)
}

fn stat(product_id: &str, name: &str, qty: f64, revenue_minor: i64, margin_minor: Option<i64>, last_sold_at: Option<&str>) -> ProductStat {
    ProductStat {
        product_id: product_id.to_string(),
        name: name.to_string(),
        qty,
        revenue_minor,
        margin_minor,
        last_sold_at: last_sold_at.map(str::to_string),
    }
}

fn sales() -> Vec<Sale> {
    vec![
        sale("2026-09-27T08:00:00.000Z", vec![s_line("p1", "Kopi Susu", 2.0, 36_000, Some(6_500)), s_line("p3", "Beras 5kg", 1.0, 68_000, Some(58_000))]),
        sale("2026-09-25T10:00:00.000Z", vec![s_line("p1", "Kopi Susu", 1.0, 18_000, Some(6_500)), s_line("p2", "Roti Bakar", 3.0, 45_000, Some(6_000))]),
        sale("2026-08-01T10:00:00.000Z", vec![s_line("p2", "Roti Bakar", 1.0, 15_000, Some(6_000)), s_line("p4", "Catering", 1.0, 50_000, None)]),
        sale("2026-08-10T10:00:00.000Z", vec![s_line("p7", "Gula Pasir", 2.0, 34_000, Some(14_500))]),
    ]
}

fn products() -> Vec<kaseer_core::repos::Product> {
    vec![
        catalog_product("p1", "Kopi Susu", true),
        catalog_product("p2", "Roti Bakar", true),
        catalog_product("p7", "Gula Pasir", true),
        catalog_product("p8", "Beras Merah", true),
        catalog_product("p4", "Catering", false),
    ]
}

#[test]
fn product_stats_ranks_by_qty_with_revenue_tiebreak_and_snapshot_names() {
    assert_eq!(
        product_stats(&sales()),
        vec![
            stat("p2", "Roti Bakar", 4.0, 60_000, Some(36_000), Some("2026-09-25T10:00:00.000Z")),
            stat("p1", "Kopi Susu", 3.0, 54_000, Some(34_500), Some("2026-09-27T08:00:00.000Z")),
            stat("p7", "Gula Pasir", 2.0, 34_000, Some(5_000), Some("2026-08-10T10:00:00.000Z")),
            stat("p3", "Beras 5kg", 1.0, 68_000, Some(10_000), Some("2026-09-27T08:00:00.000Z")),
            stat("p4", "Catering", 1.0, 50_000, None, Some("2026-08-01T10:00:00.000Z")),
        ]
    );
}

#[test]
fn best_sellers_caps_the_ranked_list_at_limit() {
    let ids: Vec<String> = best_sellers(&sales(), 2).into_iter().map(|p| p.product_id).collect();
    assert_eq!(ids, vec!["p2", "p1"]);
    assert!(best_sellers(&[], 5).is_empty());
}

#[test]
fn equal_qty_and_revenue_break_the_tie_by_name() {
    let tied = [sale("2026-09-27T09:00:00.000Z", vec![s_line("pz", "Zebra Cake", 2.0, 10_000, Some(4_000)), s_line("pa", "Alpha Tea", 2.0, 10_000, Some(4_000))])];
    let names: Vec<String> = best_sellers(&tied, 5).into_iter().map(|p| p.name).collect();
    assert_eq!(names, vec!["Alpha Tea", "Zebra Cake"]);
}

#[test]
fn revenue_sums_per_product_across_sales() {
    assert_eq!(revenue_by_product(&sales()), BTreeMap::from([
        ("p1".into(), 54_000),
        ("p2".into(), 60_000),
        ("p3".into(), 68_000),
        ("p4".into(), 50_000),
        ("p7".into(), 34_000),
    ]));
}

#[test]
fn margin_uses_per_unit_cost_times_qty_and_drops_cost_free_lines() {
    assert_eq!(margin_by_product(&sales()), BTreeMap::from([
        ("p1".into(), 34_500),
        ("p2".into(), 36_000),
        ("p3".into(), 10_000),
        ("p7".into(), 5_000),
    ]));
}

#[test]
fn margin_can_be_negative_when_cost_exceeds_net() {
    let underwater = [sale("2026-09-27T09:00:00.000Z", vec![s_line("p9", "Promo Bundle", 1.0, 5_000, Some(9_000))])];
    assert_eq!(margin_by_product(&underwater), BTreeMap::from([("p9".into(), -4_000)]));
}

fn stale(product_id: &str, name: &str, on_hand: i64, last_sold_at: Option<&str>, days: Option<i64>) -> kaseer_core::product_perf::StaleStock {
    kaseer_core::product_perf::StaleStock {
        product_id: product_id.to_string(),
        name: name.to_string(),
        on_hand,
        last_sold_at: last_sold_at.map(str::to_string),
        days_since_last_sale: days,
    }
}

#[test]
fn slow_movers_lists_stocked_tracked_items_without_an_in_window_sale() {
    let on_hand = BTreeMap::from([
        ("p1".to_string(), 5),
        ("p2".to_string(), 4),
        ("p7".to_string(), 30),
        ("p8".to_string(), 10),
        ("p4".to_string(), 99),
    ]);
    assert_eq!(slow_movers(&sales(), &products(), 30, &on_hand, NOW), vec![
        stale("p7", "Gula Pasir", 30, Some("2026-08-10T10:00:00.000Z"), Some(48)),
    ]);
}

#[test]
fn slow_movers_excludes_never_sold_out_of_window_recent_sellers_empty_and_untracked() {
    let on_hand = BTreeMap::from([
        ("p1".to_string(), 5),
        ("p2".to_string(), 4),
        ("p7".to_string(), 30),
        ("p8".to_string(), 10),
        ("p4".to_string(), 99),
    ]);
    let ids: Vec<String> = slow_movers(&sales(), &products(), 30, &on_hand, NOW).into_iter().map(|s| s.product_id).collect();
    assert!(!ids.contains(&"p8".to_string()));
    let only_p7 = BTreeMap::from([("p7".to_string(), 30)]);
    assert!(slow_movers(&sales(), &products(), 30, &only_p7, "2026-09-01T00:00:00.000Z").is_empty());
    let only_p1_zero = BTreeMap::from([("p1".to_string(), 0)]);
    assert!(slow_movers(&sales(), &products(), 30, &only_p1_zero, NOW).is_empty());
    let only_p4 = BTreeMap::from([("p4".to_string(), 99)]);
    assert!(slow_movers(&sales(), &products(), 30, &only_p4, "2026-09-01T00:00:00.000Z").is_empty());
}

#[test]
fn a_sale_exactly_at_the_cutoff_keeps_the_item_off_one_second_earlier_lands_on_it() {
    let at_cutoff = [sale("2026-08-28T12:00:00.000Z", vec![s_line("p10", "Teh Tarik", 1.0, 9_000, Some(3_000))])];
    let just_before = [sale("2026-08-28T11:59:59.999Z", vec![s_line("p10", "Teh Tarik", 1.0, 9_000, Some(3_000))])];
    let catalog = vec![catalog_product("p10", "Teh Tarik", true)];
    let on_hand = BTreeMap::from([("p10".to_string(), 7)]);
    assert!(slow_movers(&at_cutoff, &catalog, 30, &on_hand, NOW).is_empty());
    assert_eq!(slow_movers(&just_before, &catalog, 30, &on_hand, NOW), vec![
        stale("p10", "Teh Tarik", 7, Some("2026-08-28T11:59:59.999Z"), Some(30)),
    ]);
}

#[test]
fn dead_inventory_lists_never_sold_first_then_out_of_window_sellers() {
    let on_hand = BTreeMap::from([
        ("p1".to_string(), 5),
        ("p2".to_string(), 4),
        ("p7".to_string(), 30),
        ("p8".to_string(), 10),
    ]);
    assert_eq!(dead_inventory(&sales(), &on_hand, 30, NOW), vec![
        stale("p8", "p8", 10, None, None),
        stale("p7", "p7", 30, Some("2026-08-10T10:00:00.000Z"), Some(48)),
    ]);
}

#[test]
fn zero_stock_and_in_window_sellers_are_not_dead() {
    let on_hand = BTreeMap::from([("p1".to_string(), 5), ("p2".to_string(), 0)]);
    assert!(dead_inventory(&sales(), &on_hand, 30, NOW).is_empty());
}
