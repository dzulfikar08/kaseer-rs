//! Shared Sale/Customer/Product fixtures for the analytics test suites,
//! mirroring the TS oracle's per-file builders. Sales carry fixed ISO stamps
//! so every assertion is clock-free.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU32, Ordering};

use uuid::Uuid;

use kaseer_core::receipt::{Sale, SaleLine, SaleTender};
use kaseer_core::repos::{Customer, Product};
use kaseer_core::reports::{ms_to_iso, parse_ms};

pub const MS_DAY: i64 = 86_400_000;

static SEQ: AtomicU32 = AtomicU32::new(0);

fn next_seq() -> u32 {
    SEQ.fetch_add(1, Ordering::Relaxed) + 1
}

/// One sale line; `unit_minor` mirrors net so line math stays visible.
pub fn line(product_id: &str, name: &str, qty: f64, net_minor: i64, cost_minor: Option<i64>) -> SaleLine {
    SaleLine {
        product_id: product_id.to_string(),
        name: name.to_string(),
        sku: format!("{product_id}-SKU"),
        qty,
        unit_minor: net_minor,
        cost_minor,
        discount_minor: 0,
        net_minor,
        tax_minor: 0,
        tax_rate_id: None,
    }
}

/// Bundle-test line shape: net = unit * qty.
pub fn unit_line(product_id: &str, name: &str, qty: f64) -> SaleLine {
    SaleLine {
        product_id: product_id.to_string(),
        name: name.to_string(),
        sku: product_id.to_uppercase(),
        qty,
        unit_minor: 10_000,
        cost_minor: None,
        discount_minor: 0,
        net_minor: 10_000 * qty as i64,
        tax_minor: 0,
        tax_rate_id: None,
    }
}

/// Sale with lines, no customer; totals collapse to the line sum.
pub fn line_sale(at: &str, lines: Vec<SaleLine>) -> Sale {
    let net_minor: i64 = lines.iter().map(|l| l.net_minor).sum();
    bare_sale(at, None, lines, net_minor, 0)
}

/// Sale with lines and a customer (analytics-style: snapshot = customer id).
pub fn customer_line_sale(at: &str, lines: Vec<SaleLine>, customer_id: Option<&str>) -> Sale {
    let net_minor: i64 = lines.iter().map(|l| l.net_minor).sum();
    let mut sale = bare_sale(at, customer_id, lines, net_minor, 0);
    sale.customer_name_snapshot = customer_id.map(str::to_string);
    sale
}

/// Line-less sale at a fixed total (reports/segments/winback shape).
pub fn bare_sale(at: &str, customer_id: Option<&str>, lines: Vec<SaleLine>, net_minor: i64, tax_minor: i64) -> Sale {
    let seq = next_seq();
    Sale {
        id: Uuid::new_v4().to_string(),
        number: format!("KSR-TEST-{seq:04}"),
        at: at.to_string(),
        currency: "IDR".into(),
        shift_id: None,
        customer_id: customer_id.map(str::to_string),
        customer_name_snapshot: None,
        lines,
        tenders: vec![],
        gross_minor: net_minor,
        discount_minor: 0,
        net_minor,
        tax_minor,
        total_minor: net_minor + tax_minor,
        change_minor: 0,
        points_earned: 0,
        points_redeemed: 0,
        tax_by_rate_id: Default::default(),
    }
}

/// Settled cash sale in the db.test.ts shape (copilot fixtures).
pub fn fixture_sale(number: &str, at: &str, customer_id: Option<&str>, lines: Vec<SaleLine>) -> Sale {
    let total_minor: i64 = lines.iter().map(|l| l.qty as i64 * l.unit_minor).sum();
    let mut sale = bare_sale(at, customer_id, lines, total_minor, 0);
    sale.number = number.to_string();
    sale.tenders = vec![SaleTender { method: kaseer_core::payments::TenderMethod::Cash, applied_minor: total_minor, given_minor: Some(total_minor) }];
    sale
}

pub fn customer(id: &str, name: &str, phone: &str, points: i64, created_at: &str) -> Customer {
    Customer { id: id.to_string(), name: name.to_string(), phone: phone.to_string(), points, created_at: created_at.to_string() }
}

pub fn catalog_product(id: &str, name: &str, track_stock: bool) -> Product {
    Product {
        id: id.to_string(),
        sku: format!("{id}-SKU"),
        name: name.to_string(),
        category: "Tests".into(),
        price_minor: 0,
        cost_minor: 0,
        tax_rate_id: None,
        track_stock,
        low_stock_at: 0,
        archived: false,
        description: String::new(),
        prep_notes: String::new(),
        allergens: String::new(),
        route_kitchen: false,
        image_path: String::new(),
    }
}

/// ISO stamp exactly `days` (may be fractional) before `now_iso`.
pub fn days_before(now_iso: &str, days: f64) -> String {
    ms_to_iso(parse_ms(now_iso).unwrap() - (days * MS_DAY as f64) as i64)
}

pub fn parse_ms_pub(iso: &str) -> i64 {
    parse_ms(iso).unwrap()
}

pub fn iso_from_ms_pub(ms: i64) -> String {
    ms_to_iso(ms)
}
