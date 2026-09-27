//! Cart: a pure value plus pure transforms. Every mutation returns a new
//! cart; totals are computed in exact integer space.

use std::sync::atomic::{AtomicI64, Ordering};

use serde::{Deserialize, Serialize};

use crate::currency::{round_half_up_div, Currency};
use crate::money::allocate;
use crate::tax::{resolve_rate, tax_on, TaxRate};

/// Line or order discount: percent in basis points or a capped amount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Discount {
    Percent { bps: i64 },
    Amount { minor: i64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CartLine {
    pub id: String,
    pub product_id: String,
    pub name: String,
    pub sku: String,
    pub unit_minor: i64,
    /// Positive, at most three decimals (weighted goods).
    pub qty: f64,
    pub tax_rate_id: Option<String>,
    pub discount: Option<Discount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cart {
    pub currency: String,
    pub lines: Vec<CartLine>,
    pub order_discount: Option<Discount>,
    pub customer_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CartError {
    #[error("quantity must be positive: {0}")]
    MustBePositive(f64),
    #[error("quantity too large: {0}")]
    TooLarge(f64),
    #[error("quantity exceeds 3 decimals: {0}")]
    Precision(f64),
}

static LINE_SEQ: AtomicI64 = AtomicI64::new(0);

fn new_line_id() -> String {
    let seq = LINE_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    format!("line-{}", seq)
}

pub fn empty_cart(currency: &str) -> Cart {
    Cart {
        currency: currency.to_string(),
        lines: vec![],
        order_discount: None,
        customer_id: None,
    }
}

fn assert_qty(qty: f64) -> Result<(), CartError> {
    if !qty.is_finite() || qty <= 0.0 {
        return Err(CartError::MustBePositive(qty));
    }
    if qty > 9_999.0 {
        return Err(CartError::TooLarge(qty));
    }
    if (qty * 1_000.0).round() != qty * 1_000.0 {
        return Err(CartError::Precision(qty));
    }
    Ok(())
}

fn round_qty(q: f64) -> f64 {
    (q * 1_000.0).round() / 1_000.0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddableProduct {
    pub id: String,
    pub name: String,
    pub sku: String,
    pub price_minor: i64,
    pub tax_rate_id: Option<String>,
}

pub fn add_to_cart(cart: &Cart, product: &AddableProduct, qty: f64) -> Result<Cart, CartError> {
    assert_qty(qty)?;
    let mut lines = cart.lines.clone();
    if let Some(existing) = lines.iter_mut().find(|l| {
        l.product_id == product.id
            && l.discount.is_none()
            && l.note.is_none()
            && l.tax_rate_id == product.tax_rate_id
            && l.unit_minor == product.price_minor
    }) {
        existing.qty = round_qty(existing.qty + qty);
        return Ok(Cart { lines, ..cart.clone() });
    }
    let line = CartLine {
        id: new_line_id(),
        product_id: product.id.clone(),
        name: product.name.clone(),
        sku: product.sku.clone(),
        unit_minor: product.price_minor,
        qty,
        tax_rate_id: product.tax_rate_id.clone(),
        discount: None,
        note: None,
    };
    lines.push(line);
    Ok(Cart { lines, ..cart.clone() })
}

pub fn set_line_qty(cart: &Cart, line_id: &str, qty: f64) -> Result<Cart, CartError> {
    if qty <= 0.0 {
        return Ok(remove_line(cart, line_id));
    }
    assert_qty(qty)?;
    let mut lines = cart.lines.clone();
    if let Some(line) = lines.iter_mut().find(|l| l.id == line_id) {
        line.qty = round_qty(qty);
    }
    Ok(Cart { lines, ..cart.clone() })
}

pub fn remove_line(cart: &Cart, line_id: &str) -> Cart {
    let lines: Vec<CartLine> = cart.lines.iter().filter(|l| l.id != line_id).cloned().collect();
    Cart { lines, ..cart.clone() }
}

pub fn set_line_discount(cart: &Cart, line_id: &str, discount: Option<Discount>) -> Cart {
    let lines = cart
        .lines
        .iter()
        .map(|l| {
            if l.id == line_id {
                let mut l = l.clone();
                l.discount = discount.clone();
                l
            } else {
                l.clone()
            }
        })
        .collect();
    Cart { lines, ..cart.clone() }
}

pub fn set_line_note(cart: &Cart, line_id: &str, note: Option<String>) -> Cart {
    let lines = cart
        .lines
        .iter()
        .map(|l| {
            if l.id == line_id {
                let mut l = l.clone();
                l.note = note.clone();
                l
            } else {
                l.clone()
            }
        })
        .collect();
    Cart { lines, ..cart.clone() }
}

pub fn set_order_discount(cart: &Cart, discount: Option<Discount>) -> Cart {
    Cart { order_discount: discount, ..cart.clone() }
}

pub fn set_cart_customer(cart: &Cart, customer_id: Option<String>) -> Cart {
    Cart { customer_id, ..cart.clone() }
}

pub fn clear_cart(cart: &Cart) -> Cart {
    empty_cart(&cart.currency)
}

fn discount_to_minor(discount: &Discount, base_minor: i64, _currency: &Currency) -> i64 {
    match discount {
        Discount::Percent { bps } => round_half_up_div(base_minor * bps, 10_000),
        Discount::Amount { minor } => (*minor).min(base_minor),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineTotals {
    pub gross_minor: i64,
    pub line_discount_minor: i64,
    pub order_discount_minor: i64,
    pub net_minor: i64,
    pub tax_minor: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CartTotals {
    /// Parallel to the cart's lines.
    pub lines: Vec<LineTotals>,
    pub gross_minor: i64,
    pub discount_minor: i64,
    pub net_minor: i64,
    pub tax_minor: i64,
    /// What the customer owes: net plus exclusive tax; inclusive tax is inside net.
    pub total_minor: i64,
    pub tax_by_rate_id: std::collections::BTreeMap<String, i64>,
    pub item_count: f64,
}

struct WorkingLine<'a> {
    line: &'a CartLine,
    gross_minor: i64,
    line_discount_minor: i64,
    after_discount_minor: i64,
}

pub fn compute_totals(cart: &Cart, rates: &[TaxRate], currency: &Currency) -> CartTotals {
    // Gross in exact integer space: qty carries at most 3 decimals, so scale
    // once and half-up divide. No float drift can touch a line total.
    let working: Vec<WorkingLine> = cart
        .lines
        .iter()
        .map(|line| {
            let qty_scaled = (line.qty * 1_000.0).round() as i64;
            let gross_minor = round_half_up_div(line.unit_minor * qty_scaled, 1_000);
            let line_discount_minor = line
                .discount
                .as_ref()
                .map(|d| discount_to_minor(d, gross_minor, currency))
                .unwrap_or(0);
            WorkingLine { line, gross_minor, line_discount_minor, after_discount_minor: gross_minor - line_discount_minor }
        })
        .collect();

    let order_base: i64 = working.iter().map(|w| w.after_discount_minor).sum();
    let order_total = cart
        .order_discount
        .as_ref()
        .map(|d| discount_to_minor(d, order_base, currency))
        .unwrap_or(0);
    let weights: Vec<i64> = working.iter().map(|w| w.after_discount_minor).collect();
    let allocated = allocate(&crate::money::money(order_total, currency.code), &weights);

    let lines: Vec<LineTotals> = working
        .iter()
        .zip(allocated)
        .map(|(w, alloc)| {
            let net_minor = w.after_discount_minor - alloc.minor;
            let tax_minor = tax_on(net_minor, resolve_rate(w.line.tax_rate_id.as_deref(), rates), currency);
            LineTotals {
                gross_minor: w.gross_minor,
                line_discount_minor: w.line_discount_minor,
                order_discount_minor: alloc.minor,
                net_minor,
                tax_minor,
            }
        })
        .collect();

    let gross_minor: i64 = working.iter().map(|w| w.gross_minor).sum();
    let line_discount_total: i64 = working.iter().map(|w| w.line_discount_minor).sum();
    let net_minor: i64 = lines.iter().map(|l| l.net_minor).sum();
    let tax_minor: i64 = lines.iter().map(|l| l.tax_minor).sum();

    let mut tax_by_rate_id = std::collections::BTreeMap::new();
    let mut exclusive_tax = 0i64;
    for (cart_line, line_total) in cart.lines.iter().zip(&lines) {
        let rate = resolve_rate(cart_line.tax_rate_id.as_deref(), rates);
        let Some(rate) = rate else { continue };
        if rate.bps == 0 {
            continue;
        }
        *tax_by_rate_id.entry(rate.id.clone()).or_insert(0) += line_total.tax_minor;
        if !rate.inclusive {
            exclusive_tax += line_total.tax_minor;
        }
    }

    CartTotals {
        lines,
        gross_minor,
        discount_minor: order_total + line_discount_total,
        net_minor,
        tax_minor,
        total_minor: net_minor + exclusive_tax,
        tax_by_rate_id,
        item_count: cart.lines.iter().map(|l| l.qty).sum(),
    }
}
