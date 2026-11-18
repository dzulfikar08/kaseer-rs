//! The register's Grow loop: add-on pairs, WhatsApp numbers and receipts,
//! win-back codes from the menu pull, and the new sale fields end to end.

mod common;

use common::{catalog_product, customer, line, line_sale};
use kaseer_core::currency::IDR;
use kaseer_core::db::open_repo;
use kaseer_core::grow::{addon_pairs, phone_key, receipt_text, wa_phone, whatsapp_url};
use kaseer_core::menu::{apply_menu, parse_menu};
use kaseer_core::receipt::{AddonOffer, Sale};
use kaseer_core::repos::{Product, normalize_code};

fn priced(id: &str, price_minor: i64) -> Product {
    Product { price_minor, ..catalog_product(id, id, false) }
}

fn baskets(n: usize, ids: &[&str]) -> Vec<Sale> {
    (0..n)
        .map(|_| line_sale("2026-11-10T03:00:00.000Z", ids.iter().map(|id| line(id, id, 1.0, 1_000, None)).collect()))
        .collect()
}

#[test]
fn the_cheaper_item_is_the_add_on_and_weak_pairs_are_dropped() {
    let products = [priced("nasi", 25_000), priced("teh", 6_000), priced("kopi", 18_000), priced("beras", 68_000)];
    let mut sales = baskets(10, &["nasi", "teh"]);
    sales.extend(baskets(10, &["nasi"]));
    sales.extend(baskets(20, &["kopi"]));
    // Together often, but only 5% of kopi baskets: not worth asking.
    sales.extend(baskets(5, &["kopi", "beras"]));
    sales.extend(baskets(75, &["kopi"]));
    let pairs = addon_pairs(&sales, &products);
    assert_eq!(pairs.len(), 1);
    let p = &pairs["nasi"];
    assert_eq!((p.partner_id.as_str(), p.attach_pct), ("teh", 50));
}

#[test]
fn archived_products_are_never_offered() {
    let mut teh = priced("teh", 6_000);
    teh.archived = true;
    let mut sales = baskets(10, &["nasi", "teh"]);
    sales.extend(baskets(10, &["nasi"]));
    sales.extend(baskets(20, &["kopi"]));
    assert!(addon_pairs(&sales, &[priced("nasi", 25_000), teh, priced("kopi", 18_000)]).is_empty());
}

#[test]
fn whatsapp_numbers_follow_the_clouds_rules() {
    for raw in ["0812-3456-7001", "+62 812 3456 7001", "812 3456 7001", "+62 (0)812 3456 7001", "0062 812 3456 7001"] {
        assert_eq!(wa_phone(raw, "IDR").as_deref(), Some("6281234567001"), "{raw}");
    }
    assert_eq!(wa_phone("(021) 555-1234", "IDR"), None);
    assert_eq!(wa_phone("123", "IDR"), None);
    assert_eq!(wa_phone("+1 415 555 0100", "USD").as_deref(), Some("14155550100"));
    assert_eq!(phone_key("0812 3456 7001"), phone_key("+6281234567001"));
}

#[test]
fn the_receipt_reads_well_on_whatsapp() {
    let mut sale = line_sale("2026-11-10T03:00:00.000Z", vec![line("kopi", "Kopi Susu", 2.0, 36_000, None)]);
    sale.number = "KSR-1".into();
    sale.total_minor = 36_000;
    let text = receipt_text(&sale, "Kedai", &IDR);
    assert!(text.starts_with("*Kedai*\nKSR-1\n\n2 × Kopi Susu  Rp"), "{text}");
    assert!(text.ends_with("Terima kasih!"));
    assert_eq!(whatsapp_url("6281", "a b&c"), "https://wa.me/6281?text=a%20b%26c");
}

#[test]
fn codes_arrive_with_the_menu_and_a_sale_redeems_one_once() {
    let repo = open_repo(":memory:").unwrap();
    let body = br#"{"v":1,"rev":3,"products":[],
        "growth":{"addonPrompt":false,"askPhone":true,"winbackDiscountPct":15},
        "offers":[{"code":"K7QM3X","customerId":"cus-sari","customerName":"Sari","customerPhone":"081234567002","pct":15,"expiresAt":"2099-01-01T00:00:00.000Z"},
                  {"code":"OLDOLD","customerId":"cus-budi","pct":10,"expiresAt":"2000-01-01T00:00:00.000Z"}]}"#;
    apply_menu(&repo, &parse_menu(body).unwrap()).unwrap();

    let config = repo.settings().unwrap().config;
    assert!(!config.addon_prompt && config.ask_phone);
    assert_eq!(config.winback_discount_pct, 15);
    assert!(config.profit_guard && config.min_margin_pct == 10, "an older cloud keeps the default guard");

    let now = "2026-11-17T00:00:00.000Z";
    let offer = repo.live_winback_offer(" k7qm-3x ", now).unwrap().expect("typed loosely, still found");
    assert_eq!((offer.customer_id.as_str(), offer.pct), ("cus-sari", 15));
    assert!(repo.live_winback_offer("OLDOLD", now).unwrap().is_none(), "expired");
    assert_eq!(repo.live_winback_offer_for("cus-sari", now).unwrap().map(|o| o.code), Some("K7QM3X".into()));

    repo.upsert_product(&priced("kopi", 18_000)).unwrap();
    repo.upsert_customer(&customer("cus-sari", "Sari", "081234567002", 0, now)).unwrap();
    let mut sale = line_sale(now, vec![line("kopi", "Kopi", 1.0, 16_200, None)]);
    sale.customer_id = Some("cus-sari".into());
    sale.offer_code = Some(normalize_code("k7qm3x"));
    sale.addon_offers = vec![AddonOffer { anchor_id: "kopi".into(), partner_id: "roti".into(), accepted: false }];
    repo.insert_sale(&sale, &Default::default(), 0).unwrap();
    assert!(repo.live_winback_offer("K7QM3X", now).unwrap().is_none(), "redeemed");

    // The fields survive the ledger and reach the sync payload.
    let back = repo.sale_by_id(&sale.id).unwrap().unwrap();
    assert_eq!(back.offer_code.as_deref(), Some("K7QM3X"));
    assert_eq!(back.addon_offers, sale.addon_offers);

    // A later pull still listing the code (cloud not synced yet) keeps it redeemed here.
    apply_menu(&repo, &parse_menu(body).unwrap()).unwrap();
    assert!(repo.live_winback_offer("K7QM3X", now).unwrap().is_none());
}

#[test]
fn a_customer_is_found_by_any_spelling_of_their_number() {
    let repo = open_repo(":memory:").unwrap();
    repo.upsert_customer(&customer("cus-1", "Sari", "0812-3456-7002", 0, "2026-11-01T00:00:00.000Z")).unwrap();
    assert_eq!(repo.customer_by_phone("+62 812 3456 7002").unwrap().map(|c| c.id), Some("cus-1".into()));
    assert!(repo.customer_by_phone("0899 0000 0000").unwrap().is_none());
}

#[test]
fn a_menu_pull_writes_only_the_grow_keys_and_the_clouds_pairs() {
    let repo = open_repo(":memory:").unwrap();
    // The web companion keeps its own key (menuRev) in the same config.
    repo.conn().execute("UPDATE settings SET config_json = '{\"lostAfterDays\":45,\"menuRev\":7}' WHERE id = 1", []).unwrap();
    let body = br#"{"v":1,"rev":4,"products":[],
        "growth":{"addonPrompt":true,"askPhone":false,"winbackDiscountPct":12,"profitGuard":false,"minMarginPct":25},
        "addons":[{"anchorId":"kopi","partnerId":"roti","attachPct":36}]}"#;
    apply_menu(&repo, &parse_menu(body).unwrap()).unwrap();
    let raw: String = repo.conn().query_row("SELECT config_json FROM settings WHERE id = 1", [], |r| r.get(0)).unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(json["menuRev"], 7, "{raw}");
    assert_eq!(json["lostAfterDays"], 45);
    let config = repo.settings().unwrap().config;
    assert!(!config.ask_phone);
    assert_eq!(config.winback_discount_pct, 12);
    assert!(!config.profit_guard);
    assert_eq!(config.min_margin_pct, 25);
    assert_eq!(config.cloud_addons.len(), 1);
    assert_eq!((config.cloud_addons[0].partner_id.as_str(), config.cloud_addons[0].attach_pct), ("roti", 36));
}
