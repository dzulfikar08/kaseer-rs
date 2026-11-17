//! A demo ledger for trying the Grow loop at the register: 30 days of
//! café sales with real pairings (Kopi Susu + Roti Bakar, Nasi Goreng +
//! Es Teh), a regular, and a live win-back code for her.
//!
//!   cargo run -p kaseer-core --example grow_demo -- /path/to/kaseer.db
//!   KASEER_DATA_DIR=/path/to cargo run -p kaseer

use std::collections::HashMap;

use kaseer_core::db::open_repo;
use kaseer_core::receipt::{Sale, SaleLine};
use kaseer_core::repos::{Customer, WinbackOfferRow};

fn iso(days_ago: i64, minute: i64) -> String {
    let now = chrono::Utc::now() - chrono::Duration::days(days_ago) - chrono::Duration::minutes(minute);
    now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: grow_demo <path/to/kaseer.db>");
    let repo = open_repo(&path).expect("open ledger");
    let products: HashMap<String, kaseer_core::repos::Product> =
        repo.products(false, None).unwrap().into_iter().map(|p| (p.name.clone(), p)).collect();
    let id = |name: &str| products.get(name).unwrap_or_else(|| panic!("seed product {name}")).clone();

    if repo.customer_by_id("cus-sari").unwrap().is_some() {
        println!("demo ledger already seeded at {path}");
        if repo.active_shift().unwrap().is_none() {
            repo.open_shift(100_000, "IDR").unwrap();
        }
        return;
    }
    repo.upsert_customer(&Customer {
        id: "cus-sari".into(),
        name: "Sari Wulandari".into(),
        phone: "0812-3456-7002".into(),
        points: 40,
        created_at: iso(90, 0),
    })
    .unwrap();
    repo.replace_winback_offers(&[WinbackOfferRow {
        code: "K7QM3X".into(),
        customer_id: "cus-sari".into(),
        customer_name: "Sari Wulandari".into(),
        customer_phone: "0812-3456-7002".into(),
        pct: 10,
        expires_at: iso(-30, 0),
    }])
    .unwrap();

    let baskets: [&[&str]; 6] = [
        &["Kopi Susu Gula Aren", "Roti Bakar Coklat"],
        &["Kopi Susu Gula Aren"],
        &["Nasi Goreng Spesial", "Es Teh Manis"],
        &["Nasi Goreng Spesial"],
        &["Americano"],
        &["Ayam Geprek + Nasi", "Es Jeruk"],
    ];
    let mut n = 0;
    for day in 1..=28 {
        for (i, basket) in baskets.iter().enumerate() {
            n += 1;
            let lines: Vec<SaleLine> = basket
                .iter()
                .map(|name| {
                    let p = id(name);
                    SaleLine {
                        product_id: p.id.clone(),
                        name: p.name.clone(),
                        sku: p.sku.clone(),
                        qty: 1.0,
                        unit_minor: p.price_minor,
                        cost_minor: Some(p.cost_minor),
                        discount_minor: 0,
                        net_minor: p.price_minor,
                        tax_minor: 0,
                        tax_rate_id: None,
                    }
                })
                .collect();
            let total: i64 = lines.iter().map(|l| l.net_minor).sum();
            let sale = Sale {
                id: format!("demo-{n}"),
                number: format!("DEMO-{n:04}"),
                at: iso(day, i as i64 * 7),
                currency: "IDR".into(),
                shift_id: None,
                customer_id: None,
                customer_name_snapshot: None,
                lines,
                tenders: vec![],
                gross_minor: total,
                discount_minor: 0,
                net_minor: total,
                tax_minor: 0,
                total_minor: total,
                change_minor: 0,
                points_earned: 0,
                points_redeemed: 0,
                tax_by_rate_id: Default::default(),
                offer_code: None,
                addon_offers: vec![],
            };
            repo.insert_sale(&sale, &HashMap::new(), 0).unwrap();
        }
    }
    if repo.active_shift().unwrap().is_none() {
        repo.open_shift(100_000, "IDR").unwrap();
    }
    println!("demo ledger at {path}: {n} sales, customer Sari with code K7QM3X");
}
