//! Sale snapshot + thermal receipt rendering. A Sale is immutable once
//! written: prices, taxes, and store info are snapshotted so a later
//! product or settings edit never rewrites history.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::currency::Currency;
use crate::money::format;
use crate::payments::TenderMethod;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaleLine {
    pub product_id: String,
    pub name: String,
    pub sku: String,
    pub qty: f64,
    pub unit_minor: i64,
    /// Cost captured at sale time (margin analytics); omitted on legacy rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_minor: Option<i64>,
    pub discount_minor: i64,
    pub net_minor: i64,
    pub tax_minor: i64,
    pub tax_rate_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaleTender {
    pub method: TenderMethod,
    pub applied_minor: i64,
    /// Cash handed over; present on cash tenders only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub given_minor: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sale {
    pub id: String,
    /// Human number, e.g. KSR-20260926-0001.
    pub number: String,
    pub at: String,
    pub currency: String,
    pub shift_id: Option<String>,
    pub customer_id: Option<String>,
    pub customer_name_snapshot: Option<String>,
    pub lines: Vec<SaleLine>,
    pub tenders: Vec<SaleTender>,
    pub gross_minor: i64,
    pub discount_minor: i64,
    pub net_minor: i64,
    pub tax_minor: i64,
    pub total_minor: i64,
    pub change_minor: i64,
    pub points_earned: i64,
    pub points_redeemed: i64,
    pub tax_by_rate_id: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StoreInfo {
    pub name: String,
    pub address: Option<String>,
    pub footer: Option<String>,
}

impl StoreInfo {
    pub fn named(name: &str) -> StoreInfo {
        StoreInfo { name: name.to_string(), ..Default::default() }
    }
}

/// Daily sequence number, e.g. KSR-20260926-0001.
pub fn sale_number(date_iso: &str, seq: i64, prefix: &str) -> String {
    let day = date_iso.get(..10).unwrap_or(date_iso).replace('-', "");
    format!("{}-{}-{:04}", prefix, day, seq)
}

fn center(text: &str, width: usize) -> String {
    let text_len = text.chars().count();
    let pad = (width.saturating_sub(text_len)) / 2;
    format!("{}{}", " ".repeat(pad), text)
}

fn split_columns(left: &str, right: &str, width: usize) -> String {
    let space = (width.saturating_sub(left.chars().count() + right.chars().count())).max(1);
    format!("{}{}{}", left, " ".repeat(space), right)
}

fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut lines: Vec<String> = vec![];
    let mut line = String::new();
    for word in words {
        if line.is_empty() {
            line = word.to_string();
        } else if line.chars().count() + 1 + word.chars().count() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(std::mem::take(&mut line));
            line = word.to_string();
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Render a 80mm-class thermal receipt as plain text (default 42 columns).
/// Pure text so it prints, previews, and diffs identically everywhere.
pub fn render_receipt_text(sale: &Sale, store: &StoreInfo, currency: &Currency, width: usize) -> String {
    let mut out: Vec<String> = vec![];
    let rule = "-".repeat(width);
    let fmt = |minor: i64| -> String {
        format(&crate::money::money(minor, currency.code), Some(currency)).expect("sale currency matches receipt currency")
    };

    out.push(center(&store.name.to_uppercase(), width));
    if let Some(address) = &store.address {
        for line in wrap_words(address, width) {
            out.push(center(&line, width));
        }
    }
    out.push(rule.clone());

    let stamp = sale.at.chars().take(16).collect::<String>().replacen('T', " ", 1);
    let number = format!("#{}", sale.number);
    out.push(split_columns(&stamp, &number, width));

    for line in &sale.lines {
        for text_line in wrap_words(&line.name, width) {
            out.push(text_line);
        }
        let qty_str = if line.qty.fract() == 0.0 {
            format!("{}", line.qty as i64)
        } else {
            format!("{:.3}", line.qty)
        };
        let detail = format!("  {} x {}", qty_str, fmt(line.unit_minor));
        let net = fmt(line.net_minor);
        if detail.chars().count() + net.chars().count() <= width {
            out.push(split_columns(&detail, &net, width));
        } else {
            out.push(net);
        }
        if line.discount_minor > 0 {
            let discount = format!("-{}", fmt(line.discount_minor));
            out.push(split_columns("    discount", &discount, width));
        }
    }

    out.push(rule.clone());
    if sale.discount_minor > 0 {
        out.push(split_columns("Subtotal", &fmt(sale.gross_minor), width));
        let discount = format!("-{}", fmt(sale.discount_minor));
        out.push(split_columns("Discount", &discount, width));
    }
    for (rate_id, minor) in &sale.tax_by_rate_id {
        if *minor > 0 {
            let label = format!("Tax ({}, incl)", rate_id);
            out.push(split_columns(&label, &fmt(*minor), width));
        }
    }
    out.push(split_columns("TOTAL", &fmt(sale.total_minor), width));

    for t in &sale.tenders {
        let label = match t.method {
            TenderMethod::Cash if t.given_minor.is_some() => {
                format!("Cash (given {})", fmt(t.given_minor.unwrap_or(0)))
            }
            other => other.as_str().to_uppercase(),
        };
        out.push(split_columns(&label, &fmt(t.applied_minor), width));
    }
    if sale.change_minor > 0 {
        out.push(split_columns("Change", &fmt(sale.change_minor), width));
    }

    if sale.points_earned > 0 || sale.points_redeemed > 0 {
        out.push(rule.clone());
        if sale.points_redeemed > 0 {
            out.push(split_columns("Points redeemed", &sale.points_redeemed.to_string(), width));
        }
        if sale.points_earned > 0 {
            let earned = format!("+{}", sale.points_earned);
            out.push(split_columns("Points earned", &earned, width));
        }
    }

    out.push(rule);
    if let Some(footer) = &store.footer {
        for line in wrap_words(footer, width) {
            out.push(center(&line, width));
        }
    }

    out.join("\n")
}
