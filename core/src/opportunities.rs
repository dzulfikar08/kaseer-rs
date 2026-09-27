//! Sales opportunity detection (Phase 3): weekend demand skew and restock
//! timing, both pure functions over settled sales. Rates are integer basis
//! points; the one decimal that leaves the module is daysOfCover, rounded
//! at the boundary with round-half-up division. The restock window ends on
//! the most recent sale's date, never the wall clock, so results stay
//! deterministic and testable.
//!
//! On-hand stock maps are `BTreeMap<String, i64>` (product id -> units),
//! the same convention as product-perf.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::receipt::Sale;
use crate::reports::{day_key, day_of_week, day_start_ms, ms_day_key, MS_DAY};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeekendPattern {
    pub product_id: String,
    pub name: String,
    /// Share of the product's sales that fell on Saturday/Sunday, basis points 0..10000.
    pub weekend_rate: i64,
    pub verdict: WeekendVerdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeekendVerdict {
    WeekendHeavy,
}

impl WeekendVerdict {
    pub fn as_str(self) -> &'static str {
        "weekend-heavy"
    }
}

/// Round-half-up division in float space (the TS oracle's sold quantities
/// may be fractional), floored to integer tenths of a day.
fn round_half_up_div_f(num: f64, den: f64) -> i64 {
    ((2.0 * num + den) / (2.0 * den)).floor() as i64
}

/// Products whose demand skews to weekends: at least `min_sales` sales
/// contain the product and strictly more than half of those land on
/// Saturday or Sunday. Sorted by weekendRate desc, then productId.
pub fn weekend_pattern(sales: &[Sale], min_sales: i64) -> Vec<WeekendPattern> {
    let mut totals: HashMap<&str, i64> = HashMap::new();
    let mut weekend_totals: HashMap<&str, i64> = HashMap::new();
    let mut first_names: HashMap<&str, &str> = HashMap::new();
    for sale in sales {
        let dow = day_of_week(&sale.at);
        let on_weekend = dow == 0 || dow == 6;
        let mut ids: HashSet<&str> = HashSet::new();
        for line in &sale.lines {
            ids.insert(&line.product_id);
            first_names.entry(&line.product_id).or_insert(&line.name);
        }
        for id in ids {
            *totals.entry(id).or_insert(0) += 1;
            if on_weekend {
                *weekend_totals.entry(id).or_insert(0) += 1;
            }
        }
    }
    let mut rows: Vec<WeekendPattern> = vec![];
    for (product_id, total) in totals {
        let weekend_count = weekend_totals.get(product_id).copied().unwrap_or(0);
        if total < min_sales || weekend_count * 2 <= total {
            continue;
        }
        rows.push(WeekendPattern {
            product_id: product_id.to_string(),
            name: first_names.get(product_id).copied().unwrap_or(product_id).to_string(),
            weekend_rate: crate::currency::round_half_up_div(weekend_count * 10_000, total),
            verdict: WeekendVerdict::WeekendHeavy,
        });
    }
    rows.sort_by(|a, b| b.weekend_rate.cmp(&a.weekend_rate).then(a.product_id.cmp(&b.product_id)));
    rows
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestockVerdict {
    ReorderNow,
    Soon,
    Ok,
}

impl RestockVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            RestockVerdict::ReorderNow => "reorder now",
            RestockVerdict::Soon => "soon",
            RestockVerdict::Ok => "ok",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RestockSuggestion {
    pub product_id: String,
    pub name: String,
    /// Units sold inside the window.
    pub sold_in_window: f64,
    /// Stock snapshot handed in by the caller.
    pub on_hand: i64,
    /// on_hand over daily velocity (soldInWindow / daysWindow), rounded to one decimal day (e.g. 3.5).
    pub days_of_cover: f64,
    /// Under 7 days of cover: 'reorder now'; under 14: 'soon'; otherwise 'ok'.
    pub verdict: RestockVerdict,
}

/// Restock urgency from recent velocity: qty sold in the window divided by
/// the window length. The window is the `days_window` calendar dates ending
/// on the most recent sale's date (inclusive both ends). Only products with
/// stock on hand and at least one unit sold in the window come back, most
/// urgent first (daysOfCover asc, then productId); 'ok' rows are included
/// so callers can show the full picture and filter by verdict.
pub fn restock_suggestions(sales: &[Sale], on_hand: &BTreeMap<String, i64>, days_window: i64) -> Vec<RestockSuggestion> {
    if sales.is_empty() || days_window <= 0 {
        return vec![];
    }
    let mut anchor_key = "";
    for sale in sales {
        let key = day_key(&sale.at);
        if key > anchor_key {
            anchor_key = key;
        }
    }
    let Some(anchor_ms) = day_start_ms(anchor_key) else {
        return vec![];
    };
    let start_key = ms_day_key(anchor_ms - (days_window - 1) * MS_DAY);

    let mut sold: HashMap<&str, f64> = HashMap::new();
    let mut first_names: HashMap<&str, &str> = HashMap::new();
    for sale in sales {
        let key = day_key(&sale.at);
        if key < start_key.as_str() || key > anchor_key {
            continue;
        }
        for line in &sale.lines {
            *sold.entry(&line.product_id).or_insert(0.0) += line.qty;
            first_names.entry(&line.product_id).or_insert(&line.name);
        }
    }

    let mut rows: Vec<RestockSuggestion> = vec![];
    for (product_id, sold_in_window) in sold {
        let stock = on_hand.get(product_id).copied().unwrap_or(0);
        if stock <= 0 {
            continue;
        }
        // Cover in tenths of a day keeps the whole computation integer;
        // the single /10 at the end is the API boundary.
        let tenths = round_half_up_div_f(stock as f64 * days_window as f64 * 10.0, sold_in_window);
        rows.push(RestockSuggestion {
            product_id: product_id.to_string(),
            name: first_names.get(product_id).copied().unwrap_or(product_id).to_string(),
            sold_in_window,
            on_hand: stock,
            days_of_cover: tenths as f64 / 10.0,
            verdict: if tenths < 70 {
                RestockVerdict::ReorderNow
            } else if tenths < 140 {
                RestockVerdict::Soon
            } else {
                RestockVerdict::Ok
            },
        });
    }
    rows.sort_by(|a, b| {
        a.days_of_cover
            .total_cmp(&b.days_of_cover)
            .then(a.product_id.cmp(&b.product_id))
    });
    rows
}
