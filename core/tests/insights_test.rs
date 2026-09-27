//! Ported from kaseer/src/data/customer-insights.test.ts — visit counts,
//! recency, lifetime value, and the returning/lost split against a real
//! in-memory repo (now comes from the clock inside insights_for, so stamps
//! are written relative to now with half-day offsets to keep floor stable).

mod common;

use std::collections::HashMap;

use common::{bare_sale, customer};
use kaseer_core::db::open_repo;
use kaseer_core::insights::insights_for;
use kaseer_core::repos::Repo;

const MS_DAY: i64 = 24 * 60 * 60 * 1000;

/// ISO timestamp `days` in the past, offset half a day so floor() is stable.
fn days_ago(days: i64) -> String {
    common::iso_from_ms_pub(chrono::Utc::now().timestamp_millis() - (days * MS_DAY + MS_DAY / 2))
}

fn add_customer(repo: &Repo, name: &str) -> kaseer_core::repos::Customer {
    repo.upsert_customer(&customer(&format!("cust-{name}"), name, "0812000", 0, &common::iso_from_ms_pub(chrono::Utc::now().timestamp_millis())))
        .unwrap()
}

fn buy(repo: &Repo, customer_id: &str, at: &str, net_minor: i64) {
    // Lifetime math must read net, so the fixture carries +111 tax in total.
    repo.insert_sale(&bare_sale(at, Some(customer_id), vec![], net_minor, 111), &HashMap::new(), 0).unwrap();
}

fn insight_for(repo: &Repo, customer_id: &str) -> kaseer_core::insights::CustomerInsight {
    insights_for(repo, 30)
        .unwrap()
        .into_iter()
        .find(|i| i.customer_id == customer_id)
        .unwrap_or_else(|| panic!("no insight for {customer_id}"))
}

#[test]
fn customer_with_no_sales_zero_visits_never_lost_null_recency() {
    let repo = open_repo(":memory:").unwrap();
    let c = add_customer(&repo, "Walk-in Wanda");
    let insight = insight_for(&repo, &c.id);
    assert_eq!(insight.visits, 0);
    assert_eq!(insight.last_visit_at, None);
    assert_eq!(insight.lifetime_net_minor, 0);
    assert!(!insight.returning);
    assert!(!insight.lost);
    assert_eq!(insight.days_since_last_visit, None);
}

#[test]
fn one_recent_visit_counted_not_returning_not_lost() {
    let repo = open_repo(":memory:").unwrap();
    let c = add_customer(&repo, "New Nina");
    let at = days_ago(2);
    buy(&repo, &c.id, &at, 12_500);
    let insight = insight_for(&repo, &c.id);
    assert_eq!(insight.visits, 1);
    assert_eq!(insight.last_visit_at.as_deref(), Some(at.as_str()));
    assert_eq!(insight.lifetime_net_minor, 12_500);
    assert!(!insight.returning);
    assert!(!insight.lost);
    assert_eq!(insight.days_since_last_visit, Some(2));
}

#[test]
fn multiple_visits_lifetime_sums_net_not_total() {
    let repo = open_repo(":memory:").unwrap();
    let c = add_customer(&repo, "Regular Rara");
    let at1 = days_ago(10);
    let at2 = days_ago(5);
    let at3 = days_ago(1);
    buy(&repo, &c.id, &at1, 20_000);
    buy(&repo, &c.id, &at2, 8_000);
    buy(&repo, &c.id, &at3, 12_500);
    let insight = insight_for(&repo, &c.id);
    assert_eq!(insight.visits, 3);
    assert!(insight.returning);
    assert_eq!(insight.last_visit_at.as_deref(), Some(at3.as_str()));
    assert_eq!(insight.days_since_last_visit, Some(1));
    // Net only: totalMinor includes +111 tax per sale, which must not leak in.
    assert_eq!(insight.lifetime_net_minor, 40_500);
}

#[test]
fn silent_past_default_30_days_is_lost_window_override_widens_it() {
    let repo = open_repo(":memory:").unwrap();
    let gone = add_customer(&repo, "Gone Gita");
    let fresh = add_customer(&repo, "Fresh Fajar");
    buy(&repo, &gone.id, &days_ago(31), 5_000);
    buy(&repo, &fresh.id, &days_ago(29), 5_000);

    let gone_default = insight_for(&repo, &gone.id);
    assert!(gone_default.lost);
    assert_eq!(gone_default.days_since_last_visit, Some(31));

    let fresh_default = insight_for(&repo, &fresh.id);
    assert!(!fresh_default.lost);

    // Wider window keeps a 40-day-old customer from being marked lost.
    let restless = add_customer(&repo, "Restless Rio");
    buy(&repo, &restless.id, &days_ago(40), 5_000);
    assert!(insight_for(&repo, &restless.id).lost);
    let restless_wide = insights_for(&repo, 45)
        .unwrap()
        .into_iter()
        .find(|i| i.customer_id == restless.id)
        .unwrap();
    assert!(!restless_wide.lost);
}

#[test]
fn one_insight_per_customer_none_invented() {
    let repo = open_repo(":memory:").unwrap();
    let alya = add_customer(&repo, "Alya");
    let beni = add_customer(&repo, "Beni");
    let citra = add_customer(&repo, "Citra");
    buy(&repo, &alya.id, &days_ago(3), 1_000);
    let insights = insights_for(&repo, 30).unwrap();
    // repo.customers() is name-ordered, so the passed-in order is preserved.
    assert_eq!(
        insights.iter().map(|i| i.customer_id.clone()).collect::<Vec<_>>(),
        vec![alya.id, beni.id, citra.id]
    );
}
