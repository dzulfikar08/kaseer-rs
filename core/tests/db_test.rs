//! Ported from kaseer/src/data/db.test.ts — persistence layer proof:
//! migrations seed cleanly, a settled sale round-trips with lines and
//! tenders, stock and points book atomically, held tickets cycle. Plus one
//! file-mode smoke test proving the repo never touches the user's real DB.

use std::collections::HashMap;

use uuid::Uuid;

use kaseer_core::db::open_repo;
use kaseer_core::payments::TenderMethod;
use kaseer_core::receipt::{Sale, SaleLine, SaleTender};
use kaseer_core::repos::{KnowledgePatch, RepoError};

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
        offer_code: None,
        addon_offers: vec![],
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

/// A v2-shaped ledger as the web companion and the terminal left it before
/// schema v3: the v1 catalog (pinned here — migrations must never rewrite
/// it) plus the two v2 columns. `migrate` then walks 2→3 over these rows.
fn v2_fixture_conn(dir: &tempfile::TempDir) -> rusqlite::Connection {
    let path = dir.path().join("v2-ledger.db");
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE settings (
           id INTEGER PRIMARY KEY CHECK (id = 1),
           store_name TEXT NOT NULL,
           address TEXT NOT NULL DEFAULT '',
           receipt_footer TEXT NOT NULL DEFAULT '',
           currency_code TEXT NOT NULL,
           loyalty_json TEXT NOT NULL,
           config_json TEXT NOT NULL DEFAULT '{}'
         );
         CREATE TABLE tax_rates (
           id TEXT PRIMARY KEY, name TEXT NOT NULL, bps INTEGER NOT NULL, inclusive INTEGER NOT NULL
         );
         CREATE TABLE products (
           id TEXT PRIMARY KEY,
           sku TEXT NOT NULL UNIQUE,
           name TEXT NOT NULL,
           category TEXT NOT NULL DEFAULT '',
           price_minor INTEGER NOT NULL,
           cost_minor INTEGER NOT NULL DEFAULT 0,
           tax_rate_id TEXT REFERENCES tax_rates(id),
           track_stock INTEGER NOT NULL DEFAULT 1,
           low_stock_at INTEGER NOT NULL DEFAULT 0,
           archived INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE sales (
           id TEXT PRIMARY KEY,
           number TEXT NOT NULL UNIQUE,
           at TEXT NOT NULL,
           currency TEXT NOT NULL,
           shift_id TEXT,
           customer_id TEXT,
           customer_name_snapshot TEXT,
           gross_minor INTEGER NOT NULL,
           discount_minor INTEGER NOT NULL,
           net_minor INTEGER NOT NULL,
           tax_minor INTEGER NOT NULL,
           total_minor INTEGER NOT NULL,
           change_minor INTEGER NOT NULL,
           points_earned INTEGER NOT NULL DEFAULT 0,
           points_redeemed INTEGER NOT NULL DEFAULT 0,
           tax_by_rate_json TEXT NOT NULL DEFAULT '{}'
         );
         INSERT INTO settings (id, store_name, currency_code, loyalty_json) VALUES (1, 'V2 Store', 'IDR', '{}');
         INSERT INTO products (id, sku, name, category, price_minor) VALUES
           ('p-americano', '8991002100028', 'Americano', 'Coffee', 15000),
           ('p-rice', '9999999999999', 'Not Seeded', 'Food', 12000);
         PRAGMA user_version = 2;",
    )
    .unwrap();
    conn
}

#[test]
fn migration_v2_to_v3_preserves_rows_and_seeds_knowledge_and_kitchen_tables() {
    let dir = tempfile::tempdir().unwrap();
    let conn = v2_fixture_conn(&dir);
    kaseer_core::db::migrate(&conn).unwrap();

    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    // migrate() always walks to the latest step (v5 today); this test's
    // subject is what the v3 step did to these rows on the way.
    assert_eq!(version, 5);

    // Both pre-existing rows survive untouched apart from the new columns.
    let (n, non_seeded): (i64, String) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE((SELECT description FROM products WHERE sku = '9999999999999'), 'missing') FROM products",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(n, 2);
    assert_eq!(non_seeded, "");

    // The prepared SKU gets exactly the web companion's demo copy.
    let (description, prep, allergens, routed): (String, String, String, i64) = conn
        .query_row(
            "SELECT description, prep_notes, allergens, route_kitchen FROM products WHERE sku = '8991002100028'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(description, "Double shot of espresso lengthened with hot water. Served black.");
    assert_eq!(prep, "Pull 2 shots, add hot water to the 8oz line. Serve immediately.");
    assert_eq!(allergens, "none");
    assert_eq!(routed, 1);

    // Kitchen tables exist for the web companion even though the terminal
    // never writes them.
    for table in ["kitchen_tickets", "kitchen_ticket_items"] {
        let found: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(found, 1, "{table} must exist after v3");
    }
}

#[test]
fn fresh_ledger_seeds_the_same_knowledge_as_the_web_companion() {
    let repo = open_repo(":memory:").unwrap();
    let americano = repo
        .products(false, None)
        .unwrap()
        .into_iter()
        .find(|p| p.sku == "8991002100028")
        .unwrap();
    assert_eq!(
        americano.description,
        "Double shot of espresso lengthened with hot water. Served black."
    );
    assert_eq!(americano.prep_notes, "Pull 2 shots, add hot water to the 8oz line. Serve immediately.");
    assert_eq!(americano.allergens, "none");
    assert!(americano.route_kitchen);

    for sku in ["8991002100011", "8991002101018", "8991002101025", "8991002101032", "8991002101049", "8991002102017"] {
        let p = repo
            .products(false, None)
            .unwrap()
            .into_iter()
            .find(|p| p.sku == sku)
            .unwrap_or_else(|| panic!("{sku} must be in the seed catalog"));
        assert!(p.route_kitchen, "{sku} must route to the kitchen");
        assert!(!p.description.is_empty());
    }

    // Groceries stay knowledge-free.
    let rice = repo
        .products(false, None)
        .unwrap()
        .into_iter()
        .find(|p| p.sku == "8991002104015")
        .unwrap();
    assert!(!rice.route_kitchen);
    assert_eq!(rice.description, "");
}

#[test]
fn update_knowledge_merges_the_patch_and_unknown_ids_error() {
    let repo = open_repo(":memory:").unwrap();
    let americano_id = repo
        .products(false, None)
        .unwrap()
        .into_iter()
        .find(|p| p.sku == "8991002100028")
        .unwrap()
        .id;

    // A partial patch keeps every omitted field at its seeded value.
    let updated = repo
        .update_knowledge(&americano_id, &KnowledgePatch { prep_notes: Some("Test pull one shot.".into()), ..Default::default() })
        .unwrap();
    assert_eq!(updated.prep_notes, "Test pull one shot.");
    assert_eq!(updated.description, "Double shot of espresso lengthened with hot water. Served black.");
    assert_eq!(updated.allergens, "none");
    assert!(updated.route_kitchen);

    // The stored row matches, including the untouched fields.
    let stored = repo.product_by_id(&americano_id).unwrap().unwrap();
    assert_eq!(stored.prep_notes, "Test pull one shot.");
    assert!(stored.route_kitchen);

    let unknown = repo.update_knowledge("no-such-id", &KnowledgePatch::default());
    assert!(matches!(unknown, Err(RepoError::NotFound(_))));
}

/// A v3-shaped ledger (the catalog as v3 left it: knowledge columns, no
/// photo path) with two rows — one seeded-SKU, one foreign — so the v4
/// step must alter the table without touching the rows.
fn v3_fixture_conn(dir: &tempfile::TempDir) -> rusqlite::Connection {
    let path = dir.path().join("v3-ledger.db");
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE products (
           id TEXT PRIMARY KEY,
           sku TEXT NOT NULL UNIQUE,
           name TEXT NOT NULL,
           category TEXT NOT NULL DEFAULT '',
           price_minor INTEGER NOT NULL,
           cost_minor INTEGER NOT NULL DEFAULT 0,
           tax_rate_id TEXT,
           track_stock INTEGER NOT NULL DEFAULT 1,
           low_stock_at INTEGER NOT NULL DEFAULT 0,
           archived INTEGER NOT NULL DEFAULT 0,
           description TEXT NOT NULL DEFAULT '',
           prep_notes TEXT NOT NULL DEFAULT '',
           allergens TEXT NOT NULL DEFAULT '',
           route_kitchen INTEGER NOT NULL DEFAULT 0
         );
         INSERT INTO products (id, sku, name, category, price_minor, description, route_kitchen) VALUES
           ('p-americano', '8991002100028', 'Americano', 'Coffee', 15000, 'Double shot of espresso lengthened with hot water. Served black.', 1),
           ('p-rice', '9999999999999', 'Not Seeded', 'Food', 12000, '', 0);
         PRAGMA user_version = 3;",
    )
    .unwrap();
    conn
}

#[test]
fn migration_v3_to_v4_preserves_rows_and_adds_image_path() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("v3-ledger.db");
    {
        let conn = v3_fixture_conn(&dir);
        kaseer_core::db::migrate(&conn).unwrap();

        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, 5);

        // Both pre-existing rows survive with knowledge intact.
        let (n, description, routed): (i64, String, i64) = conn
            .query_row(
                "SELECT COUNT(*),
                        COALESCE((SELECT description FROM products WHERE sku = '8991002100028'), 'missing'),
                        COALESCE((SELECT route_kitchen FROM products WHERE sku = '8991002100028'), -1)
                 FROM products",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(
            description,
            "Double shot of espresso lengthened with hot water. Served black."
        );
        assert_eq!(routed, 1);
    }

    let repo = open_repo(db_path.to_str().unwrap()).unwrap();
    let products = repo.products(true, None).unwrap();
    assert_eq!(products.len(), 2);
    for p in &products {
        assert_eq!(p.image_path, "", "v4 must default every image_path to ''");
    }
}

#[test]
fn fresh_v4_ledger_seeds_products_with_empty_image_paths() {
    let repo = open_repo(":memory:").unwrap();
    let products = repo.products(true, None).unwrap();
    assert_eq!(products.len(), 24);
    assert!(
        products.iter().all(|p| p.image_path.is_empty()),
        "a fresh terminal ledger has no photos until the first menu pull"
    );

    // The full v4 column set round-trips through upsert_product.
    let mut p = products[0].clone();
    p.image_path = "images/abc123.jpg".into();
    let stored = repo.upsert_product(&p).unwrap();
    assert_eq!(stored.image_path, "images/abc123.jpg");
    assert_eq!(repo.product_by_id(&p.id).unwrap().unwrap().image_path, "images/abc123.jpg");
}

#[test]
fn settings_round_trip_the_companions_camelcase_json() {
    let repo = open_repo(":memory:").unwrap();

    // A web companion-seeded settings row: camelCase loyalty keys and a
    // config_json carrying the companion's menuRev.
    repo.conn()
        .execute(
            "UPDATE settings SET loyalty_json = ?1, config_json = ?2 WHERE id = 1",
            rusqlite::params![
                r#"{"enabled":true,"stepMinor":10000,"pointsPerStep":2,"pointValueMinor":150}"#,
                r#"{"lostAfterDays":45,"menuRev":7}"#
            ],
        )
        .unwrap();
    let settings = repo.settings().unwrap();
    assert!(settings.loyalty.enabled);
    assert_eq!(settings.loyalty.step_minor, 10_000);
    assert_eq!(settings.loyalty.points_per_step, 2);
    assert_eq!(settings.loyalty.point_value_minor, 150);
    assert_eq!(settings.config.lost_after_days, 45);

    // Legacy kiosk-written snake_case ledgers stay readable.
    repo.conn()
        .execute(
            "UPDATE settings SET loyalty_json = ?1 WHERE id = 1",
            rusqlite::params![r#"{"enabled":false,"step_minor":5000,"points_per_step":1,"point_value_minor":50}"#],
        )
        .unwrap();
    let legacy = repo.settings().unwrap().loyalty;
    assert!(!legacy.enabled);
    assert_eq!(legacy.step_minor, 5_000);

    // A kiosk settings save writes the camelCase shape the web parses.
    let mut next = settings.clone();
    next.loyalty = kaseer_core::loyalty::LoyaltyConfig {
        enabled: true,
        step_minor: 20_000,
        points_per_step: 3,
        point_value_minor: 200,
    };
    repo.update_settings(&next).unwrap();
    let (loyalty_json, config_json): (String, String) = repo
        .conn()
        .query_row("SELECT loyalty_json, config_json FROM settings WHERE id = 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert!(loyalty_json.contains("\"stepMinor\":20000"), "{loyalty_json}");
    assert!(!loyalty_json.contains("step_minor"), "{loyalty_json}");
    let web_view: serde_json::Value = serde_json::from_str(&config_json).unwrap();
    assert_eq!(web_view["lostAfterDays"], 45, "{config_json}");
}
