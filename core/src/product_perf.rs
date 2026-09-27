//! Product performance (Phase 2 BI): pure functions over settled sales plus
//! the catalog. Names come from the sale-line snapshot for sold items and
//! from the product list for stock that never moved. Money is integer minor
//! units; margin is per-line net minus per-unit cost times qty, and lines
//! without a captured cost are skipped (legacy rows).
//!
//! On-hand stock maps are `BTreeMap<String, i64>` (product id -> units):
//! iteration is key-ordered so dead-inventory output never depends on
//! caller map construction order.

use std::collections::{BTreeMap, HashMap};

use crate::receipt::Sale;
use crate::repos::Product;
use crate::reports::{parse_ms, MS_DAY};

#[derive(Debug, Clone, PartialEq)]
pub struct ProductStat {
    pub product_id: String,
    pub name: String,
    /// Units sold across the window.
    pub qty: f64,
    /// Post-discount, pre-tax line revenue.
    pub revenue_minor: i64,
    /// revenue minus cost; None when no line in the window captured a cost.
    pub margin_minor: Option<i64>,
    /// Newest sale line in the whole history, window aside.
    pub last_sold_at: Option<String>,
}

struct Agg {
    name: String,
    qty: f64,
    revenue_minor: i64,
    margin_minor: f64,
    has_cost: bool,
    last_sold_at: Option<String>,
}

/// Math.round parity at the minor-unit boundary: margin math runs in float
/// space (qty is f64, mirroring the TS oracle) and rounds half-up once,
/// on the way out.
fn round_half(x: f64) -> i64 {
    (x + 0.5).floor() as i64
}

fn aggregate(sales: &[Sale]) -> HashMap<&str, Agg> {
    let mut by_id: HashMap<&str, Agg> = HashMap::new();
    for sale in sales {
        for line in &sale.lines {
            let agg = by_id.entry(&line.product_id).or_insert_with(|| Agg {
                name: line.name.clone(),
                qty: 0.0,
                revenue_minor: 0,
                margin_minor: 0.0,
                has_cost: false,
                last_sold_at: None,
            });
            agg.qty += line.qty;
            agg.revenue_minor += line.net_minor;
            if let Some(cost) = line.cost_minor {
                agg.margin_minor += line.net_minor as f64 - cost as f64 * line.qty;
                agg.has_cost = true;
            }
            if agg.last_sold_at.as_ref().is_none_or(|seen| sale.at.as_str() > seen.as_str()) {
                agg.last_sold_at = Some(sale.at.clone());
            }
        }
    }
    by_id
}

fn to_stat(product_id: &str, agg: Agg) -> ProductStat {
    ProductStat {
        product_id: product_id.to_string(),
        name: agg.name,
        qty: agg.qty,
        revenue_minor: agg.revenue_minor,
        margin_minor: agg.has_cost.then_some(round_half(agg.margin_minor)),
        last_sold_at: agg.last_sold_at,
    }
}

/// Every product sold in the window, busiest first; name, then id, breaks ties deterministically.
pub fn product_stats(sales: &[Sale]) -> Vec<ProductStat> {
    let mut stats: Vec<ProductStat> = aggregate(sales)
        .into_iter()
        .map(|(product_id, agg)| to_stat(product_id, agg))
        .collect();
    stats.sort_by(|a, b| {
        b.qty.total_cmp(&a.qty)
            .then(b.revenue_minor.cmp(&a.revenue_minor))
            .then(a.name.cmp(&b.name))
            .then(a.product_id.cmp(&b.product_id))
    });
    stats
}

/// Top `limit` products by units sold in the window.
pub fn best_sellers(sales: &[Sale], limit: usize) -> Vec<ProductStat> {
    let mut stats = product_stats(sales);
    stats.truncate(limit);
    stats
}

/// Product id -> revenue in minor units (all sales handed in).
pub fn revenue_by_product(sales: &[Sale]) -> BTreeMap<String, i64> {
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    for sale in sales {
        for line in &sale.lines {
            *out.entry(line.product_id.clone()).or_insert(0) += line.net_minor;
        }
    }
    out
}

/// Product id -> margin in minor units, counting cost-carrying lines only.
/// Products whose lines all lack a captured cost are omitted entirely.
pub fn margin_by_product(sales: &[Sale]) -> BTreeMap<String, i64> {
    let mut out: BTreeMap<String, f64> = BTreeMap::new();
    for sale in sales {
        for line in &sale.lines {
            if let Some(cost) = line.cost_minor {
                *out.entry(line.product_id.clone()).or_insert(0.0) += line.net_minor as f64 - cost as f64 * line.qty;
            }
        }
    }
    out.into_iter().map(|(id, margin)| (id, round_half(margin))).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleStock {
    pub product_id: String,
    pub name: String,
    pub on_hand: i64,
    pub last_sold_at: Option<String>,
    /// Whole days since the last sale; None when the item never sold.
    pub days_since_last_sale: Option<i64>,
}

fn last_sold_by_product(sales: &[Sale]) -> HashMap<&str, &str> {
    let mut last: HashMap<&str, &str> = HashMap::new();
    for sale in sales {
        for line in &sale.lines {
            match last.get(line.product_id.as_str()) {
                Some(&seen) if sale.at.as_str() <= seen => {}
                _ => {
                    last.insert(&line.product_id, &sale.at);
                }
            }
        }
    }
    last
}

/// Stocked catalog items that have sold before but not within the last
/// `days` days: the shelves that still move, just slowly.
pub fn slow_movers(
    sales: &[Sale],
    products: &[Product],
    days: i64,
    on_hand: &BTreeMap<String, i64>,
    now_iso: &str,
) -> Vec<StaleStock> {
    let now_ms = parse_ms(now_iso).unwrap_or(0);
    let cutoff = now_ms - days * MS_DAY;
    let last = last_sold_by_product(sales);
    let mut out: Vec<StaleStock> = vec![];
    for product in products {
        if !product.track_stock {
            continue;
        }
        let held = on_hand.get(&product.id).copied().unwrap_or(0);
        if held <= 0 {
            continue;
        }
        let Some(last_sold_at) = last.get(product.id.as_str()).map(|s| s.to_string()) else {
            continue;
        };
        if parse_ms(&last_sold_at).is_some_and(|sold| sold >= cutoff) {
            continue;
        }
        let days_since = parse_ms(&last_sold_at).map(|sold| ((now_ms - sold) / MS_DAY).max(0));
        out.push(StaleStock {
            product_id: product.id.clone(),
            name: product.name.clone(),
            on_hand: held,
            last_sold_at: Some(last_sold_at),
            days_since_last_sale: days_since,
        });
    }
    out.sort_by(|a, b| {
        a.last_sold_at
            .as_deref()
            .unwrap_or("")
            .cmp(b.last_sold_at.as_deref().unwrap_or(""))
    });
    out
}

/// Stock on hand with zero sales inside the window: includes items that
/// never sold at all. `on_hand` keys the catalog down to what is physically
/// held, so anything in the map with a positive count qualifies. Names fall
/// back to the product id: this entry point sees only the stock map.
pub fn dead_inventory(sales: &[Sale], on_hand: &BTreeMap<String, i64>, days: i64, now_iso: &str) -> Vec<StaleStock> {
    let now_ms = parse_ms(now_iso).unwrap_or(0);
    let cutoff = now_ms - days * MS_DAY;
    let last = last_sold_by_product(sales);
    let mut out: Vec<StaleStock> = vec![];
    for (product_id, held) in on_hand {
        if *held <= 0 {
            continue;
        }
        let last_sold_at = last.get(product_id.as_str()).map(|s| s.to_string());
        if last_sold_at.as_deref().is_some_and(|at| parse_ms(at).is_some_and(|sold| sold >= cutoff)) {
            continue;
        }
        let days_since = last_sold_at.as_deref().and_then(parse_ms).map(|sold| ((now_ms - sold) / MS_DAY).max(0));
        out.push(StaleStock {
            product_id: product_id.clone(),
            name: product_id.clone(),
            on_hand: *held,
            last_sold_at: last_sold_at.clone(),
            days_since_last_sale: days_since,
        });
    }
    out.sort_by(|a, b| {
        a.last_sold_at
            .as_deref()
            .unwrap_or("")
            .cmp(b.last_sold_at.as_deref().unwrap_or(""))
            .then(a.product_id.cmp(&b.product_id))
    });
    out
}
