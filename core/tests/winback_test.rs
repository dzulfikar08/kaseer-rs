//! Ported from kaseer/src/core/winback.test.ts — fixtures go through a real
//! in-memory Repo and insert_sale with controlled `at` stamps, so lost
//! boundaries, window membership, and export serialization are asserted
//! exactly against a fixed NOW, no clock reliance.

mod common;

use std::collections::HashMap;

use common::{bare_sale, customer};
use kaseer_core::db::open_repo;
use kaseer_core::receipt::Sale;
use kaseer_core::repos::{Customer, Repo};
use kaseer_core::winback::{reengagement_csv, whatsapp_ready, winback_list, WinbackEntry};

const NOW: &str = "2026-09-27T12:00:00.000Z";

fn days_before_now(days: f64) -> String {
    common::days_before(NOW, days)
}

fn sale_at(at: &str, customer_id: Option<&str>) -> Sale {
    bare_sale(at, customer_id, vec![], 10_000, 0)
}

fn add_customer(repo: &Repo, name: &str) -> Customer {
    repo.upsert_customer(&customer(&format!("cust-{name}"), name, "0812000", 0, "2026-01-01T00:00:00.000Z")).unwrap()
}

fn buy(repo: &Repo, customer_id: Option<&str>, at: &str) {
    repo.insert_sale(&sale_at(at, customer_id), &HashMap::new(), 0).unwrap();
}

fn ledger(repo: &Repo) -> (Vec<Sale>, Vec<Customer>) {
    (repo.sales_between("0000-01-01", "9999-12-31").unwrap(), repo.customers(None).unwrap())
}

fn entry_for(list: &[WinbackEntry], customer_id: &str) -> WinbackEntry {
    list.iter()
        .find(|e| e.customer_id == customer_id)
        .cloned()
        .unwrap_or_else(|| panic!("no win-back entry for {customer_id}"))
}

#[test]
fn exact_lost_boundary_last_visit_on_cutoff_not_lost_one_day_past_is() {
    let repo = open_repo(":memory:").unwrap();
    let edge = add_customer(&repo, "Edge");
    let gone = add_customer(&repo, "Gone");
    buy(&repo, Some(&edge.id), &days_before_now(30.0));
    buy(&repo, Some(&gone.id), &days_before_now(31.0));

    let (sales, customers) = ledger(&repo);
    let list = winback_list(&sales, &customers, 30, NOW);

    assert_eq!(list.iter().map(|e| e.customer_id.clone()).collect::<Vec<_>>(), vec![gone.id.clone()]);
    assert_eq!(entry_for(&list, &gone.id).days_since, 31);
}

#[test]
fn walk_ins_and_never_purchased_customers_are_never_listed() {
    let repo = open_repo(":memory:").unwrap();
    let buyer = add_customer(&repo, "Buyer");
    add_customer(&repo, "NeverBought");
    buy(&repo, Some(&buyer.id), &days_before_now(2.0));
    buy(&repo, None, &days_before_now(60.0));

    let (sales, customers) = ledger(&repo);
    assert!(winback_list(&sales, &customers, 30, NOW).is_empty());
}

#[test]
fn frequency_drop_true_active_30_90d_ago_silent_since() {
    let repo = open_repo(":memory:").unwrap();
    let c = add_customer(&repo, "Collapser");
    for days in [85.0, 75.0, 65.0, 55.0, 45.0, 40.0, 36.0, 35.0] {
        buy(&repo, Some(&c.id), &days_before_now(days));
    }

    let (sales, customers) = ledger(&repo);
    let entry = entry_for(&winback_list(&sales, &customers, 30, NOW), &c.id);

    assert_eq!(entry.visits, 8);
    assert_eq!(entry.last_visit_at, days_before_now(35.0));
    assert_eq!(entry.previous_visits_per_week, 0.93);
    assert_eq!(entry.recent_visits_per_week, 0.0);
    assert!(entry.frequency_dropped);
}

#[test]
fn frequency_drop_false_below_four_lifetime_visits() {
    let repo = open_repo(":memory:").unwrap();
    let c = add_customer(&repo, "Sporadic");
    for days in [80.0, 50.0, 35.0] {
        buy(&repo, Some(&c.id), &days_before_now(days));
    }

    let (sales, customers) = ledger(&repo);
    let entry = entry_for(&winback_list(&sales, &customers, 30, NOW), &c.id);

    assert_eq!(entry.visits, 3);
    assert_eq!(entry.previous_visits_per_week, 0.35);
    assert_eq!(entry.recent_visits_per_week, 0.0);
    assert!(!entry.frequency_dropped);
}

#[test]
fn recent_rate_exactly_half_the_prior_rate_is_not_a_drop() {
    let repo = open_repo(":memory:").unwrap();
    let c = add_customer(&repo, "Halfway");
    for days in [85.0, 75.0, 65.0, 55.0, 45.0, 40.0, 36.0, 35.0] {
        buy(&repo, Some(&c.id), &days_before_now(days));
    }
    buy(&repo, Some(&c.id), &days_before_now(25.0));
    buy(&repo, Some(&c.id), &days_before_now(20.0));

    let (sales, customers) = ledger(&repo);
    let entry = entry_for(&winback_list(&sales, &customers, 14, NOW), &c.id);

    assert_eq!(entry.days_since, 20);
    assert_eq!(entry.previous_visits_per_week, 0.93);
    assert_eq!(entry.recent_visits_per_week, 0.47);
    assert!(!entry.frequency_dropped);
}

#[test]
fn nonzero_recent_rate_still_drops_under_short_lost_window() {
    let repo = open_repo(":memory:").unwrap();
    let c = add_customer(&repo, "Slowing");
    for days in [85.0, 75.0, 65.0, 55.0, 45.0, 40.0, 36.0, 35.0] {
        buy(&repo, Some(&c.id), &days_before_now(days));
    }
    buy(&repo, Some(&c.id), &days_before_now(21.0));

    let (sales, customers) = ledger(&repo);
    let entry = entry_for(&winback_list(&sales, &customers, 14, NOW), &c.id);

    assert_eq!(entry.previous_visits_per_week, 0.93);
    assert_eq!(entry.recent_visits_per_week, 0.23);
    assert!(entry.frequency_dropped);
}

#[test]
fn a_sale_exactly_on_the_30d_seam_counts_toward_recent_not_prior() {
    let repo = open_repo(":memory:").unwrap();
    let c = add_customer(&repo, "Seam");
    for days in [85.0, 75.0, 65.0, 55.0, 45.0, 40.0] {
        buy(&repo, Some(&c.id), &days_before_now(days));
    }
    buy(&repo, Some(&c.id), &days_before_now(30.0));

    let (sales, customers) = ledger(&repo);
    let entry = entry_for(&winback_list(&sales, &customers, 14, NOW), &c.id);

    assert_eq!(entry.last_visit_at, days_before_now(30.0));
    assert_eq!(entry.previous_visits_per_week, 0.7);
    assert_eq!(entry.recent_visits_per_week, 0.23);
}

#[test]
fn orders_longest_silent_first_name_breaking_exact_ties() {
    let repo = open_repo(":memory:").unwrap();
    let zack = add_customer(&repo, "Zack");
    let anna = add_customer(&repo, "Anna");
    let mila = add_customer(&repo, "Mila");
    buy(&repo, Some(&zack.id), &days_before_now(31.0));
    buy(&repo, Some(&anna.id), &days_before_now(90.0));
    buy(&repo, Some(&mila.id), &days_before_now(31.0));

    let (sales, customers) = ledger(&repo);
    let list = winback_list(&sales, &customers, 30, NOW);

    assert_eq!(list.iter().map(|e| e.name.clone()).collect::<Vec<_>>(), vec!["Anna", "Mila", "Zack"]);
    assert_eq!(list.iter().map(|e| e.days_since).collect::<Vec<_>>(), vec![90, 31, 31]);
}

#[test]
fn whatsapp_ready_name_phone_lines_only_for_reachable_contacts() {
    let repo = open_repo(":memory:").unwrap();
    let with_phone = repo.upsert_customer(&customer("cust-Dewi", "Dewi", "0812345", 0, "2026-01-01T00:00:00.000Z")).unwrap();
    let no_phone = repo.upsert_customer(&customer("cust-Budi", "Budi", "", 0, "2026-01-01T00:00:00.000Z")).unwrap();
    buy(&repo, Some(&with_phone.id), &days_before_now(40.0));
    buy(&repo, Some(&no_phone.id), &days_before_now(45.0));

    let (sales, customers) = ledger(&repo);
    let list = winback_list(&sales, &customers, 30, NOW);

    assert_eq!(whatsapp_ready(&list), "Dewi;0812345\n");
}

#[test]
fn whatsapp_ready_empty_list_serializes_to_empty_string() {
    assert_eq!(whatsapp_ready(&[]), "");
}

#[test]
fn reengagement_csv_header_plus_one_row_per_entry() {
    let repo = open_repo(":memory:").unwrap();
    let c = repo.upsert_customer(&customer("cust-Dewi", "Dewi", "0812345", 0, "2026-01-01T00:00:00.000Z")).unwrap();
    buy(&repo, Some(&c.id), &days_before_now(35.0));

    let (sales, customers) = ledger(&repo);
    let csv = reengagement_csv(&winback_list(&sales, &customers, 30, NOW));

    assert_eq!(
        csv,
        format!(
            "name,phone,last_visit_at,days_since,visits,previous_visits_per_week,recent_visits_per_week,frequency_dropped\nDewi,0812345,{},35,1,0.12,0,false\n",
            days_before_now(35.0)
        )
    );
}

#[test]
fn reengagement_csv_quotes_fields_with_commas_quotes_or_newlines() {
    let repo = open_repo(":memory:").unwrap();
    let c = repo
        .upsert_customer(&customer("cust-quote", "Dewi \"Sari\", Warung\n02", "0812345", 0, "2026-01-01T00:00:00.000Z"))
        .unwrap();
    buy(&repo, Some(&c.id), &days_before_now(35.0));

    let (sales, customers) = ledger(&repo);
    let csv = reengagement_csv(&winback_list(&sales, &customers, 30, NOW));

    assert!(csv.contains("\"Dewi \"\"Sari\"\", Warung\n02\""));
}
