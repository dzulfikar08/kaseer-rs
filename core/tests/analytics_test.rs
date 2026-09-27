//! Ported from kaseer/src/core/analytics.test.ts — dashboard stats + customer
//! mix. Fixed `at` stamps and exact minor-unit assertions; the clock never
//! participates (ranges are caller-supplied).

mod common;

use common::{customer, customer_line_sale, line};
use kaseer_core::analytics::{customer_mix, dashboard_stats, CustomerMix, DashboardStats};
use kaseer_core::receipt::Sale;

const NOW: &str = "2026-09-27T09:30:00.000Z";

fn sale(at: &str, lines: Vec<kaseer_core::receipt::SaleLine>, customer_id: Option<&str>) -> Sale {
    customer_line_sale(at, lines, customer_id)
}

fn kopi(net_minor: i64) -> Vec<kaseer_core::receipt::SaleLine> {
    vec![line("p1", "Kopi", 1.0, net_minor, None)]
}

#[test]
fn dashboard_stats_sums_the_three_ranges_and_averages_today_only() {
    let today = [sale("2026-09-27T02:00:00.000Z", kopi(12_000), None), sale("2026-09-27T04:00:00.000Z", kopi(7_500), None)];
    let yesterday = [sale("2026-09-26T04:00:00.000Z", vec![line("p2", "Roti", 1.0, 15_000, None)], None)];
    let this_month = [
        sale("2026-09-27T02:00:00.000Z", kopi(12_000), None),
        sale("2026-09-27T04:00:00.000Z", kopi(7_500), None),
        sale("2026-09-26T04:00:00.000Z", vec![line("p2", "Roti", 1.0, 15_000, None)], None),
        sale("2026-09-25T04:00:00.000Z", vec![line("p3", "Beras", 1.0, 40_000, None)], None),
    ];
    assert_eq!(
        dashboard_stats(&today, &yesterday, &this_month),
        DashboardStats {
            revenue_today_minor: 19_500,
            revenue_yesterday_minor: 15_000,
            revenue_this_month_minor: 74_500,
            tx_today: 2,
            avg_basket_today_minor: 9_750,
        }
    );
}

#[test]
fn empty_ranges_read_zero_and_average_basket_is_zero_without_sales() {
    assert_eq!(
        dashboard_stats(&[], &[], &[]),
        DashboardStats {
            revenue_today_minor: 0,
            revenue_yesterday_minor: 0,
            revenue_this_month_minor: 0,
            tx_today: 0,
            avg_basket_today_minor: 0,
        }
    );
}

fn customers() -> Vec<kaseer_core::repos::Customer> {
    vec![
        customer("c1", "c1", "", 0, "2026-01-10T00:00:00.000Z"),
        customer("c2", "c2", "", 0, "2026-09-27T01:00:00.000Z"),
        customer("c3", "c3", "", 0, "2026-09-26T00:00:00.000Z"),
    ]
}

#[test]
fn first_purchase_in_period_is_new_prior_purchases_make_returning() {
    let sales = [
        sale("2026-09-26T10:00:00.000Z", kopi(10_000), Some("c1")),
        sale("2026-09-27T02:00:00.000Z", kopi(10_000), Some("c1")),
        sale("2026-09-27T03:00:00.000Z", vec![line("p2", "Roti", 1.0, 12_000, None)], Some("c2")),
        sale("2026-09-27T04:00:00.000Z", vec![line("p3", "Teh", 1.0, 6_000, None)], Some("c3")),
    ];
    assert_eq!(customer_mix(&sales, &customers(), NOW), CustomerMix { new_customers: 2, returning_customers: 1 });
}

#[test]
fn walk_ins_unknown_customers_and_customers_without_purchases_never_count() {
    let sales = [
        sale("2026-09-27T02:00:00.000Z", kopi(10_000), None),
        sale("2026-09-27T03:00:00.000Z", kopi(10_000), Some("ghost")),
    ];
    assert_eq!(customer_mix(&sales, &customers(), NOW), CustomerMix { new_customers: 0, returning_customers: 0 });
}

#[test]
fn sale_exactly_at_period_start_is_inside_one_after_now_is_outside() {
    let sales = [
        sale("2026-09-27T00:00:00.000Z", kopi(10_000), Some("c1")),
        sale("2026-09-28T01:00:00.000Z", vec![line("p2", "Roti", 1.0, 12_000, None)], Some("c2")),
    ];
    assert_eq!(customer_mix(&sales, &customers(), NOW), CustomerMix { new_customers: 1, returning_customers: 0 });
}
