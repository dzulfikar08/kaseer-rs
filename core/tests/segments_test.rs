//! Ported from kaseer/src/core/segments.test.ts — segment membership on
//! fixed fixtures: every `at` stamp is an exact ISO literal so the new/lost
//! boundaries assert to the millisecond against a frozen now.

mod common;

use common::{bare_sale, customer};
use kaseer_core::receipt::Sale;
use kaseer_core::segments::{segments, Segments};

const NOW: &str = "2026-09-27T12:00:00.000Z";
const LOST_AFTER_DAYS: i64 = 30;

fn sale(at: &str, net_minor: i64, customer_id: Option<&str>) -> Sale {
    bare_sale(at, customer_id, vec![], net_minor, 0)
}

fn run(customers: &[kaseer_core::repos::Customer], sales: &[Sale]) -> Segments {
    segments(sales, customers, LOST_AFTER_DAYS, NOW)
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn exact_membership_new_loyal_lost_and_top_quartile_high_value() {
    let customers = [
        customer("c-new", "", "", 0, ""),
        customer("c-loyal", "", "", 0, ""),
        customer("c-loyal3", "", "", 0, ""),
        customer("c-lost", "", "", 0, ""),
        customer("c-zero", "", "", 0, ""),
    ];
    let mut sales: Vec<Sale> = vec![
        // c-new: a single recent purchase, 5 days old.
        sale("2026-09-22T10:00:00.000Z", 100_000, Some("c-new")),
        // c-loyal: 7 visits spread over months, first far outside the 30d new window.
        sale("2026-06-01T10:00:00.000Z", 50_000, Some("c-loyal")),
        sale("2026-07-01T10:00:00.000Z", 50_000, Some("c-loyal")),
        sale("2026-08-01T10:00:00.000Z", 50_000, Some("c-loyal")),
        sale("2026-08-20T10:00:00.000Z", 50_000, Some("c-loyal")),
        sale("2026-09-01T10:00:00.000Z", 50_000, Some("c-loyal")),
        sale("2026-09-12T10:00:00.000Z", 50_000, Some("c-loyal")),
        sale("2026-09-20T10:00:00.000Z", 50_000, Some("c-loyal")),
        // c-loyal3: returning with 3 visits, first purchase 48 days back.
        sale("2026-08-10T10:00:00.000Z", 100_000, Some("c-loyal3")),
        sale("2026-09-01T10:00:00.000Z", 100_000, Some("c-loyal3")),
        sale("2026-09-18T10:00:00.000Z", 100_000, Some("c-loyal3")),
        // c-lost: 2 visits, nothing since 2026-07-20.
        sale("2026-07-01T10:00:00.000Z", 200_000, Some("c-lost")),
        sale("2026-07-20T10:00:00.000Z", 200_000, Some("c-lost")),
        // Walk-in money never joins a segment.
        sale("2026-09-20T11:00:00.000Z", 25_000, None),
    ];
    let _ = &mut sales;
    // Quartile: 5 customers -> top ceil(5/4)=2 by lifetime net. c-lost 400k
    // and c-loyal 350k outrank c-loyal3 300k and c-new 100k.
    assert_eq!(
        run(&customers, &sales),
        Segments {
            new: ids(&["c-new"]),
            loyal: ids(&["c-loyal", "c-loyal3"]),
            lost: ids(&["c-lost"]),
            high_value: ids(&["c-loyal", "c-lost"]),
        }
    );
}

#[test]
fn a_customer_can_be_loyal_and_lost_at_once_non_purchaser_never_lost() {
    let customers = [customer("c-x", "", "", 0, ""), customer("c-idle", "", "", 0, "")];
    let sales = [
        sale("2026-04-01T10:00:00.000Z", 60_000, Some("c-x")),
        sale("2026-05-01T10:00:00.000Z", 60_000, Some("c-x")),
        sale("2026-06-01T10:00:00.000Z", 60_000, Some("c-x")),
        sale("2026-07-01T10:00:00.000Z", 60_000, Some("c-x")),
        sale("2026-07-15T10:00:00.000Z", 60_000, Some("c-x")),
    ];
    let result = run(&customers, &sales);
    assert_eq!(result.loyal, ids(&["c-x"]));
    assert_eq!(result.lost, ids(&["c-x"]));
    assert!(!result.lost.contains(&"c-idle".to_string()));
    assert!(result.new.is_empty());
    assert_eq!(result.high_value, ids(&["c-x"]));
}

#[test]
fn new_boundary_is_inclusive_at_exactly_30_days() {
    let customers = [customer("c-edge", "", "", 0, ""), customer("c-just-out", "", "", 0, "")];
    let sales = [
        sale("2026-08-28T12:00:00.000Z", 10_000, Some("c-edge")),
        sale("2026-08-28T11:59:59.999Z", 10_000, Some("c-just-out")),
    ];
    let result = run(&customers, &sales);
    assert_eq!(result.new, ids(&["c-edge"]));
    // Last visit 30 days + 1ms old is outside the 30-day window: lost.
    assert_eq!(result.lost, ids(&["c-just-out"]));
}

#[test]
fn lost_boundary_is_strict_last_visit_exactly_at_cutoff_not_lost() {
    let customers = [customer("c-edge", "", "", 0, ""), customer("c-gone", "", "", 0, "")];
    let sales = [
        sale("2026-08-28T12:00:00.000Z", 10_000, Some("c-edge")),
        sale("2026-08-28T11:59:59.999Z", 10_000, Some("c-gone")),
    ];
    assert_eq!(run(&customers, &sales).lost, ids(&["c-gone"]));
}

#[test]
fn fewer_than_four_customers_high_value_falls_back_to_four_times_average_basket() {
    let customers = [customer("c-big", "", "", 0, ""), customer("c-small", "", "", 0, "")];
    // 8 walk-in baskets of 10k drag the store average basket down to 98k.
    let mut sales: Vec<Sale> = (0..8).map(|_| sale("2026-09-21T10:00:00.000Z", 10_000, None)).collect();
    sales.push(sale("2026-09-22T10:00:00.000Z", 800_000, Some("c-big")));
    sales.push(sale("2026-09-23T10:00:00.000Z", 100_000, Some("c-small")));
    // Average basket 980k/10 = 98k; 4x = 392k, so only c-big (800k) clears it.
    assert_eq!(run(&customers, &sales).high_value, ids(&["c-big"]));
}

#[test]
fn high_value_needs_positive_lifetime_net_even_when_quartile_is_wider() {
    let customers = [
        customer("c-only", "", "", 0, ""),
        customer("c-idle1", "", "", 0, ""),
        customer("c-idle2", "", "", 0, ""),
        customer("c-idle3", "", "", 0, ""),
    ];
    let sales = [sale("2026-09-01T10:00:00.000Z", 50_000, Some("c-only"))];
    assert_eq!(run(&customers, &sales).high_value, ids(&["c-only"]));
    assert!(run(&customers, &[]).high_value.is_empty());
}

#[test]
fn lost_after_days_comes_from_config_not_a_constant() {
    let customers = [customer("c-a", "", "", 0, "")];
    let sales = [sale("2026-09-20T10:00:00.000Z", 10_000, Some("c-a"))];
    let tight = segments(&sales, &customers, 7, NOW);
    let loose = segments(&sales, &customers, 30, NOW);
    assert_eq!(tight.lost, ids(&["c-a"]));
    assert!(loose.lost.is_empty());
}

#[test]
fn same_input_twice_same_segments() {
    let customers = [customer("c-new", "", "", 0, ""), customer("c-loyal", "", "", 0, "")];
    let sales = [
        sale("2026-09-22T10:00:00.000Z", 100_000, Some("c-new")),
        sale("2026-09-01T10:00:00.000Z", 50_000, Some("c-loyal")),
        sale("2026-09-20T10:00:00.000Z", 50_000, Some("c-loyal")),
    ];
    assert_eq!(run(&customers, &sales), run(&customers, &sales));
}
