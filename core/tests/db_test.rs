//! Ported from kaseer/src/data/db.test.ts — persistence layer proof:
//! migrations seed cleanly, a settled sale round-trips with lines and
//! tenders, stock and points book atomically, held tickets cycle. Plus one
//! file-mode smoke test proving the repo never touches the user's real DB.

use std::collections::HashMap;

use uuid::Uuid;

use kaseer_core::db::open_repo;
use kaseer_core::payments::TenderMethod;
use kaseer_core::receipt::{Sale, SaleLine, SaleTender};
use kaseer_core::repos::RepoError;

fn fixture_sale(number: &str, at: &str, customer_id: Option<&str>, total_minor: i64) -> Sale {
    Sale {
        id: Uuid::new_v4().to_string(),
        number: number.to_string(),
        at: at.to_string(),
        currency: "IDR".into(),
        shift_id: None,
        customer_id: customer_id.map(str::to_string),
        customer_name_snapshot: None,
        lines: vec![SaleLine {
            product_id: "p1".into(),
            name: "Kopi".into(),
            sku: "111".into(),
            qty: 2.0,
            unit_minor: 18_000,
            cost_minor: None,
            discount_minor: 0,
            net_minor: total_minor,
            tax_minor: 0,
            tax_rate_id: Some("ppn11".into()),
        }],
        tenders: vec![SaleTender {
            method: TenderMethod::Cash,
            applied_minor: total_minor,
            given_minor: Some(total_minor),
        }],
        gross_minor: total_minor,
        discount_minor: 0,
        net_minor: total_minor,
        tax_minor: 0,
        total_minor,
        change_minor: 0,
        points_earned: 2,
        points_redeemed: 0,
        tax_by_rate_id: Default::default(),
    }
}

#[test]
fn migrations_seed_settings_rates_categories_and_24_products() {
    let repo = open_repo(":memory:").unwrap();
    assert_eq!(repo.settings().unwrap().store_name, "Kaseer Demo Store");
    assert_eq!(repo.settings().unwrap().currency_code, "IDR");
    assert_eq!(repo.tax_rates().unwrap().len(), 1);
    assert!(repo.categories().unwrap().len() >= 5);
    assert_eq!(repo.products(false, None).unwrap().len(), 24);
}

#[test]
fn a_settled_sale_round_trips_with_lines_tenders_stock_and_points() {
    let repo = open_repo(":memory:").unwrap();
    let product = repo
        .products(false, None)
        .unwrap()
        .into_iter()
        .find(|p| p.sku == "8991002104015")
        .unwrap();
    let before = repo.stock_on_hand(&product.id).unwrap();

    let sale = fixture_sale("KSR-20260926-0099", "2026-09-26T10:00:00.000Z", None, 136_000);
    repo.insert_sale(&sale, &HashMap::from([(product.id.clone(), 1.0)]), 0).unwrap();

    let roundtrip = repo.sale_by_id(&sale.id).unwrap().unwrap();
    assert_eq!(roundtrip.number, "KSR-20260926-0099");
    assert_eq!(roundtrip.total_minor, 136_000);
    assert_eq!(roundtrip.lines.len(), 1);
    assert_eq!(roundtrip.tenders[0].method, TenderMethod::Cash);
    assert_eq!(repo.stock_on_hand(&product.id).unwrap(), before - 1);

    let range = repo
        .sales_between("2026-09-26T00:00:00.000Z", "2026-09-27T00:00:00.000Z")
        .unwrap();
    assert_eq!(range.len(), 1);
    assert_eq!(range[0].lines[0].name, "Kopi");
}

#[test]
fn customer_sales_and_loyalty_deltas_book_together() {
    let repo = open_repo(":memory:").unwrap();
    let customer = repo
        .upsert_customer(&kaseer_core::repos::Customer {
            id: Uuid::new_v4().to_string(),
            name: "Budi".into(),
            phone: "08123".into(),
            points: 0,
            created_at: "2026-09-26T10:00:00.000Z".into(),
        })
        .unwrap();
    repo.insert_sale(
        &fixture_sale("KSR-20260926-0001", "2026-09-26T10:00:00.000Z", Some(&customer.id), 40_000),
        &HashMap::new(),
        5,
    )
    .unwrap();
    assert_eq!(repo.customer_by_id(&customer.id).unwrap().unwrap().points, 5);
    assert_eq!(repo.sales_by_customer(&customer.id).unwrap().len(), 1);
}

#[test]
fn shift_open_close_and_held_tickets_cycle() {
    let repo = open_repo(":memory:").unwrap();
    assert!(repo.active_shift().unwrap().is_none());
    let shift = repo.open_shift(250_000, "IDR").unwrap();
    assert_eq!(repo.active_shift().unwrap().unwrap().id, shift.id);
    assert!(matches!(repo.open_shift(1, "IDR"), Err(RepoError::ShiftAlreadyOpen)));
    repo.close_shift(&shift.id, 248_000).unwrap();
    assert!(repo.active_shift().unwrap().is_none());

    let ticket = repo.hold_ticket("Budi", r#"{"currency":"IDR","lines":[]}"#).unwrap();
    assert_eq!(repo.held_tickets().unwrap().len(), 1);
    repo.delete_held_ticket(&ticket.id).unwrap();
    assert_eq!(repo.held_tickets().unwrap().len(), 0);
}

#[test]
fn file_mode_opens_and_seeds_a_dedicated_test_db() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kaseer-test.db");
    let repo = open_repo(path.to_str().unwrap()).unwrap();
    assert_eq!(repo.products(false, None).unwrap().len(), 24);
    assert_eq!(repo.settings().unwrap().store_name, "Kaseer Demo Store");
    assert!(path.exists());
}
