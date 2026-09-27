//! Ported from kaseer/src/core/copilot.test.ts — every intent is answered
//! with exact ledger figures against an in-memory repo, pinned to a fixed
//! `now` so weeks and months never drift. 2026-09-23 is a Wednesday; this
//! week starts Monday 09-21, last week is 09-14..09-20, last month is
//! August 2026.

mod common;

use std::collections::HashMap;

use chrono::TimeZone;
use common::{customer, fixture_sale};
use kaseer_core::copilot::{answer, CopilotAnswer};
use kaseer_core::db::open_repo;
use kaseer_core::inventory::StockReason;
use kaseer_core::receipt::{Sale, SaleLine};
use kaseer_core::repos::{Customer, Product, Repo, Settings};

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc.from_utc_datetime(&chrono::NaiveDate::from_ymd_opt(2026, 9, 23).unwrap().and_hms_opt(10, 0, 0).unwrap())
}

fn ask(repo: &Repo, question: &str) -> CopilotAnswer {
    answer(repo, question, &repo.settings().unwrap(), now()).unwrap()
}

fn upsert_test_product(repo: &Repo, id: &str, sku: &str, name: &str, track_stock: bool) -> Product {
    let product = Product {
        id: id.to_string(),
        sku: sku.to_string(),
        name: name.to_string(),
        category: "Tests".into(),
        price_minor: 0,
        cost_minor: 0,
        tax_rate_id: None,
        track_stock,
        low_stock_at: 5,
        archived: false,
    };
    repo.upsert_product(&product).unwrap();
    product
}

fn upsert_test_customer(repo: &Repo, id: &str, name: &str, points: i64) -> Customer {
    let c = customer(id, name, "08123", points, "2026-01-05T02:00:00.000Z");
    repo.upsert_customer(&c).unwrap();
    c
}

fn spec_line(product_id: &str, name: &str, sku: &str, qty: f64, unit_minor: i64) -> SaleLine {
    SaleLine {
        product_id: product_id.to_string(),
        name: name.to_string(),
        sku: sku.to_string(),
        qty,
        unit_minor,
        cost_minor: None,
        discount_minor: 0,
        net_minor: qty as i64 * unit_minor,
        tax_minor: 0,
        tax_rate_id: None,
    }
}

fn insert(repo: &Repo, sale: Sale) {
    repo.insert_sale(&sale, &HashMap::new(), 0).unwrap();
}

#[test]
fn summarizes_sales_this_week_with_exact_ledger_figures() {
    let repo = open_repo(":memory:").unwrap();
    let kopi = upsert_test_product(&repo, "prod-kopi", "9001", "Kopi", false);
    let mk = |unit: i64| spec_line("prod-kopi", "Kopi", &kopi.sku, 1.0, unit);
    insert(&repo, fixture_sale("KSR-20260921-0001", "2026-09-21T04:00:00.000Z", None, vec![mk(60_000)]));
    insert(&repo, fixture_sale("KSR-20260922-0001", "2026-09-22T04:00:00.000Z", None, vec![mk(44_000)]));
    insert(&repo, fixture_sale("KSR-20260923-0001", "2026-09-23T04:00:00.000Z", None, vec![mk(50_000)]));

    let out = ask(&repo, "How were sales this week?");
    assert!(out.matched);
    assert!(out.answer.contains("Sales this week: Rp 154.000 across 3 sales"), "{}", out.answer);
    assert!(out.answer.contains("average basket Rp 51.333"), "{}", out.answer);
    assert_eq!(out.citations, vec!["this week: Rp 154.000 across 3 sales".to_string()]);
}

#[test]
fn explains_a_down_week_with_delta_and_slowest_day() {
    let repo = open_repo(":memory:").unwrap();
    let kopi = upsert_test_product(&repo, "prod-kopi", "9001", "Kopi", false);
    let mk = |qty: f64, unit: i64| spec_line("prod-kopi", "Kopi", &kopi.sku, qty, unit);
    // This week: Mon 60.000, Tue 44.000, Wed 50.000.
    insert(&repo, fixture_sale("KSR-20260921-0001", "2026-09-21T04:00:00.000Z", None, vec![mk(1.0, 60_000)]));
    insert(&repo, fixture_sale("KSR-20260922-0001", "2026-09-22T04:00:00.000Z", None, vec![mk(1.0, 44_000)]));
    insert(&repo, fixture_sale("KSR-20260923-0001", "2026-09-23T04:00:00.000Z", None, vec![mk(1.0, 50_000)]));
    // Last week: 320.000 across 3 sales.
    insert(&repo, fixture_sale("KSR-20260914-0001", "2026-09-14T04:00:00.000Z", None, vec![mk(2.0, 60_000)]));
    insert(&repo, fixture_sale("KSR-20260916-0001", "2026-09-16T04:00:00.000Z", None, vec![mk(2.0, 60_000)]));
    insert(&repo, fixture_sale("KSR-20260918-0001", "2026-09-18T04:00:00.000Z", None, vec![mk(1.0, 80_000)]));

    let out = ask(&repo, "Why are sales down this week?");
    assert!(out.matched);
    assert!(out.answer.contains("Rp 154.000"), "{}", out.answer);
    assert!(out.answer.contains("Rp 320.000 across 3 sales last week"), "{}", out.answer);
    assert!(out.answer.contains("down Rp 166.000 (52%)"), "{}", out.answer);
    assert!(out.answer.contains("Slowest day so far: Tuesday at Rp 44.000."), "{}", out.answer);
    assert!(out.citations.contains(&"this week: Rp 154.000 across 3 sales".to_string()));
    assert!(out.citations.contains(&"last week: Rp 320.000 across 3 sales".to_string()));
    assert!(out.citations.contains(&"slowest day Tuesday: Rp 44.000".to_string()));
}

#[test]
fn says_honestly_when_sales_are_not_down() {
    let repo = open_repo(":memory:").unwrap();
    let kopi = upsert_test_product(&repo, "prod-kopi", "9001", "Kopi", false);
    let mk = |unit: i64| spec_line("prod-kopi", "Kopi", &kopi.sku, 1.0, unit);
    insert(&repo, fixture_sale("KSR-20260921-0001", "2026-09-21T04:00:00.000Z", None, vec![mk(150_000)]));
    insert(&repo, fixture_sale("KSR-20260922-0001", "2026-09-22T04:00:00.000Z", None, vec![mk(150_000)]));
    insert(&repo, fixture_sale("KSR-20260916-0001", "2026-09-16T04:00:00.000Z", None, vec![mk(100_000)]));

    let out = ask(&repo, "Why are sales down this week?");
    assert!(out.matched);
    assert!(out.answer.contains("Sales are not down"), "{}", out.answer);
    assert!(out.answer.contains("Rp 300.000 across 2 sales"), "{}", out.answer);
    assert!(out.answer.contains("up Rp 200.000 (+200%)"), "{}", out.answer);
}

#[test]
fn lists_lost_customers_with_days_since_last_visit() {
    let repo = open_repo(":memory:").unwrap();
    let kopi = upsert_test_product(&repo, "prod-kopi", "9001", "Kopi", false);
    let mk = || spec_line("prod-kopi", "Kopi", &kopi.sku, 1.0, 20_000);
    let budi = upsert_test_customer(&repo, "cus-budi", "Budi", 0);
    let ani = upsert_test_customer(&repo, "cus-ani", "Ani", 0);
    let dewi = upsert_test_customer(&repo, "cus-dewi", "Dewi", 0);
    let citra = upsert_test_customer(&repo, "cus-citra", "Citra", 0);
    insert(&repo, fixture_sale("KSR-20260921-0001", "2026-09-21T04:00:00.000Z", Some(&budi.id), vec![mk()]));
    insert(&repo, fixture_sale("KSR-20260715-0001", "2026-07-15T04:00:00.000Z", Some(&dewi.id), vec![mk()]));
    insert(&repo, fixture_sale("KSR-20260801-0001", "2026-08-01T04:00:00.000Z", Some(&ani.id), vec![mk()]));
    insert(&repo, fixture_sale("KSR-20260901-0001", "2026-09-01T04:00:00.000Z", Some(&citra.id), vec![mk()]));

    let out = ask(&repo, "Which customers have not returned?");
    assert!(out.matched);
    assert!(
        out.answer.contains("2 customers have not been back in 30+ days: Dewi (70 days), Ani (53 days)"),
        "{}",
        out.answer
    );
    assert!(out.citations.contains(&"Dewi: last visit 2026-07-15, 70 days ago".to_string()));
    assert!(out.citations.contains(&"Ani: last visit 2026-08-01, 53 days ago".to_string()));
}

#[test]
fn ranks_reorder_candidates_by_days_of_cover() {
    let repo = open_repo(":memory:").unwrap();
    let kopi = upsert_test_product(&repo, "prod-kopi", "9001", "Kopi", true);
    let teh = upsert_test_product(&repo, "prod-teh", "9002", "Teh", true);
    repo.adjust_stock(&kopi.id, 12, StockReason::Receive, None, None).unwrap();
    repo.adjust_stock(&teh.id, 30, StockReason::Receive, None, None).unwrap();
    let kopi_line = |qty: f64| spec_line("prod-kopi", "Kopi", &kopi.sku, qty, 5_000);
    let teh_line = vec![spec_line("prod-teh", "Teh", &teh.sku, 10.0, 4_000)];
    // Kopi: 84 units inside the 28-day window; Teh: 10 units.
    insert(&repo, fixture_sale("KSR-20260828-0001", "2026-08-28T04:00:00.000Z", None, vec![kopi_line(28.0)]));
    insert(&repo, fixture_sale("KSR-20260921-0001", "2026-09-21T04:00:00.000Z", None, vec![kopi_line(28.0)]));
    insert(&repo, fixture_sale("KSR-20260922-0001", "2026-09-22T04:00:00.000Z", None, vec![kopi_line(28.0)]));
    insert(&repo, fixture_sale("KSR-20260901-0001", "2026-09-01T04:00:00.000Z", None, teh_line));

    let out = ask(&repo, "What should I reorder?");
    assert!(out.matched);
    assert!(
        out.answer.contains("Kopi (4 days of cover: 12 on hand, 84 sold in 28 days)"),
        "{}",
        out.answer
    );
    assert!(
        out.answer.contains("Teh (84 days of cover: 30 on hand, 10 sold in 28 days)"),
        "{}",
        out.answer
    );
    assert!(out.answer.find("Kopi").unwrap() < out.answer.find("Teh").unwrap());
    assert!(out.citations.contains(&"Kopi: 12 on hand, 84 sold last 28 days".to_string()));
}

#[test]
fn ranks_best_products_last_month_by_revenue() {
    let repo = open_repo(":memory:").unwrap();
    let kopi = upsert_test_product(&repo, "prod-kopi", "9001", "Kopi", false);
    let teh = upsert_test_product(&repo, "prod-teh", "9002", "Teh", false);
    insert(
        &repo,
        fixture_sale(
            "KSR-20260805-0001",
            "2026-08-05T04:00:00.000Z",
            None,
            vec![spec_line("prod-kopi", "Kopi", &kopi.sku, 60.0, 20_000)],
        ),
    );
    insert(
        &repo,
        fixture_sale(
            "KSR-20260820-0001",
            "2026-08-20T04:00:00.000Z",
            None,
            vec![
                spec_line("prod-kopi", "Kopi", &kopi.sku, 60.0, 20_000),
                spec_line("prod-teh", "Teh", &teh.sku, 40.0, 10_000),
            ],
        ),
    );

    let out = ask(&repo, "What were my best products last month?");
    assert!(out.matched);
    assert!(out.answer.contains("Best products last month (August 2026)"), "{}", out.answer);
    assert!(out.answer.contains("1. Kopi, 120 sold, Rp 2.400.000"), "{}", out.answer);
    assert!(out.answer.contains("2. Teh, 40 sold, Rp 400.000"), "{}", out.answer);
    assert!(out.answer.find("Kopi").unwrap() < out.answer.find("Teh").unwrap());
    assert_eq!(out.citations[0], "last month: Rp 2.800.000 net across 2 sales");
}

#[test]
fn reports_a_named_customer_points_and_lifetime_spend() {
    let repo = open_repo(":memory:").unwrap();
    let kopi = upsert_test_product(&repo, "prod-kopi", "9001", "Kopi", false);
    let mk = |unit: i64| spec_line("prod-kopi", "Kopi", &kopi.sku, 1.0, unit);
    let budi = upsert_test_customer(&repo, "cus-budi", "Budi", 120);
    insert(&repo, fixture_sale("KSR-20260921-0001", "2026-09-21T04:00:00.000Z", Some(&budi.id), vec![mk(60_000)]));
    insert(&repo, fixture_sale("KSR-20260916-0001", "2026-09-16T04:00:00.000Z", Some(&budi.id), vec![mk(100_000)]));

    let out = ask(&repo, "How many loyalty points does Budi have?");
    assert!(out.matched);
    assert!(
        out.answer.contains("Budi holds 120 loyalty points across 2 visits (Rp 160.000 lifetime net)"),
        "{}",
        out.answer
    );
    assert_eq!(out.citations, vec!["Budi: 120 points, 2 visits, Rp 160.000 lifetime net".to_string()]);

    let unnamed = ask(&repo, "How do points work for regulars?");
    assert!(unnamed.matched);
    assert!(unnamed.answer.contains("Name the customer and I will pull their full record"), "{}", unnamed.answer);
    assert!(unnamed.answer.contains("Budi 120 points"), "{}", unnamed.answer);
}

#[test]
fn admits_it_cannot_answer_questions_outside_the_ledger() {
    let repo = open_repo(":memory:").unwrap();
    let out = ask(&repo, "What is the airspeed velocity of an unladen swallow?");
    assert!(!out.matched);
    assert!(out.citations.is_empty());
    assert!(out.answer.contains("I only answer from your local ledger, no guessing"), "{}", out.answer);
    assert!(out.answer.contains("What should I reorder?"), "{}", out.answer);
    assert!(out.answer.contains("best products last month"), "{}", out.answer);
}

// Settings import guard: the answer entry point is exercised with the
// repo's own settings above; keep the type visible for screen authors.
#[allow(dead_code)]
fn settings_shape(_: &Settings) {}
