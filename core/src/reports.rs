//! Report queries over settled sales. Pure functions: the caller hands in
//! Sale snapshots, the module returns aggregates. No DB awareness.
//!
//! Calendar conventions (crate-wide, reused by analytics/product-perf/
//! opportunities/copilot): ISO stamps are read on their UTC date part
//! (`at[0..10]`), weeks roll Monday, ISO years roll on the week holding
//! Jan 4. `parse_ms`/`ms_to_iso` accept the stamp shapes the app produces:
//! RFC3339, minute precision (`2026-09-21T09:00`), and date-only. Date-only
//! stamps parse as UTC midnight, matching JS `Date.parse` semantics.

use std::collections::{BTreeMap, HashMap};

use chrono::Datelike;

use crate::currency::round_half_up_div;
use crate::payments::TenderMethod;
use crate::receipt::Sale;

pub const MS_DAY: i64 = 86_400_000;

// ── Calendar helpers (the crate's single source of stamp math) ───────────

/// Milliseconds since the epoch for an ISO stamp; `None` for malformed
/// input (the JS oracle's NaN flows through as "no bucket" behavior).
pub fn parse_ms(iso: &str) -> Option<i64> {
    use chrono::TimeZone;
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso) {
        return Some(dt.timestamp_millis());
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(iso, fmt) {
            return Some(naive.and_utc().timestamp_millis());
        }
    }
    // Date-only parses as UTC midnight, like JS Date.parse.
    chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d")
        .ok()
        .and_then(|d| chrono::Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?).timestamp_millis().into())
}

/// UTC ISO stamp with millisecond precision, matching JS toISOString.
pub fn ms_to_iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// UTC midnight of a YYYY-MM-DD key; `None` for malformed input.
pub fn day_start_ms(key: &str) -> Option<i64> {
    parse_ms(key)
}

/// Local date part of an ISO stamp, matching byHourLocal's string-slice convention.
pub fn day_key(at: &str) -> &str {
    at.get(..10).unwrap_or(at)
}

/// Date key (YYYY-MM-DD) of an epoch-milliseconds instant.
pub fn ms_day_key(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .format("%Y-%m-%d")
        .to_string()
}

/// Day of week (0 = Sunday) of an ISO stamp's date part.
pub fn day_of_week(at: &str) -> u32 {
    day_key(at)
        .parse::<chrono::NaiveDate>()
        .map(|d| d.weekday().num_days_from_sunday())
        .unwrap_or(0)
}

/// ISO week label (e.g. 2026-W39) for a date key. Weeks roll Monday; ISO
/// years roll on the week holding Jan 4 (the week's Thursday fixes both).
pub fn iso_week_key(key: &str) -> String {
    let Some(ms) = day_start_ms(key) else {
        return String::new();
    };
    let dow0 = day_of_week(key) as i64;
    let dow = if dow0 == 0 { 7 } else { dow0 }; // Mon=1 .. Sun=7
    let thursday = ms + (4 - dow) * MS_DAY;
    let (year, week) = chrono::DateTime::from_timestamp_millis(thursday)
        .map(|dt| {
            let year = dt.year() as i64;
            let jan1 = chrono::NaiveDate::from_ymd_opt(year as i32, 1, 1)
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|t| t.and_utc().timestamp_millis())
                .unwrap_or(0);
            (year, (thursday - jan1) / MS_DAY / 7 + 1)
        })
        .unwrap_or((1970, 1));
    format!("{}-W{:02}", year, week)
}

// ── Summaries ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SalesSummary {
    pub count: i64,
    pub gross_minor: i64,
    pub discount_minor: i64,
    pub net_minor: i64,
    pub tax_minor: i64,
    pub total_minor: i64,
    /// Average basket in minor units; 0 when there are no sales.
    pub avg_basket_minor: i64,
}

pub fn summarize(sales: &[Sale]) -> SalesSummary {
    let count = sales.len() as i64;
    let gross_minor: i64 = sales.iter().map(|s| s.gross_minor).sum();
    let discount_minor: i64 = sales.iter().map(|s| s.discount_minor).sum();
    let net_minor: i64 = sales.iter().map(|s| s.net_minor).sum();
    let tax_minor: i64 = sales.iter().map(|s| s.tax_minor).sum();
    let total_minor: i64 = sales.iter().map(|s| s.total_minor).sum();
    SalesSummary {
        count,
        gross_minor,
        discount_minor,
        net_minor,
        tax_minor,
        total_minor,
        avg_basket_minor: if count == 0 { 0 } else { round_half_up_div(total_minor, count) },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HourBucket {
    /// Local hour, 0-23.
    pub hour: i64,
    pub total_minor: i64,
    pub count: i64,
}

pub fn by_hour_local(sales: &[Sale]) -> Vec<HourBucket> {
    let mut buckets: BTreeMap<i64, HourBucket> = BTreeMap::new();
    for sale in sales {
        let Ok(hour) = sale.at.get(11..13).unwrap_or_default().parse::<i64>() else {
            continue;
        };
        let bucket = buckets.entry(hour).or_insert(HourBucket { hour, total_minor: 0, count: 0 });
        bucket.total_minor += sale.total_minor;
        bucket.count += 1;
    }
    buckets.into_values().collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductLine {
    pub name: String,
    pub qty: f64,
    pub net_minor: i64,
}

/// Best revenue lines first, capped at `limit`. Ties keep first-seen order.
pub fn top_products(sales: &[Sale], limit: usize) -> Vec<ProductLine> {
    let mut order: Vec<ProductLine> = vec![];
    let mut index: HashMap<String, usize> = HashMap::new();
    for sale in sales {
        for line in &sale.lines {
            let key = format!("{}:{}", line.product_id, line.name);
            let entry = index.get(&key).copied();
            match entry {
                Some(i) => {
                    order[i].qty += line.qty;
                    order[i].net_minor += line.net_minor;
                }
                None => {
                    index.insert(key, order.len());
                    order.push(ProductLine {
                        name: line.name.clone(),
                        qty: line.qty,
                        net_minor: line.net_minor,
                    });
                }
            }
        }
    }
    order.sort_by_key(|l| std::cmp::Reverse(l.net_minor));
    order.truncate(limit);
    order
}

/// Settled money per tender method, summed across `applied_minor`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PaymentMix {
    pub cash: i64,
    pub card: i64,
    pub qris: i64,
    pub transfer: i64,
    pub loyalty: i64,
}

pub fn payment_mix(sales: &[Sale]) -> PaymentMix {
    let mut mix = PaymentMix::default();
    for sale in sales {
        for t in &sale.tenders {
            match t.method {
                TenderMethod::Cash => mix.cash += t.applied_minor,
                TenderMethod::Card => mix.card += t.applied_minor,
                TenderMethod::Qris => mix.qris += t.applied_minor,
                TenderMethod::Transfer => mix.transfer += t.applied_minor,
                TenderMethod::Loyalty => mix.loyalty += t.applied_minor,
            }
        }
    }
    mix
}

// ── Period series ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodBucket {
    /// Bucket label: YYYY-MM-DD (by_day), ISO week like 2026-W39 (by_week), or YYYY-MM (by_month).
    pub day: String,
    pub total_minor: i64,
    pub count: i64,
}

fn bump_bucket(buckets: &mut BTreeMap<String, PeriodBucket>, label: String, sale: &Sale) {
    let bucket = buckets.entry(label.clone()).or_insert(PeriodBucket { day: label, total_minor: 0, count: 0 });
    bucket.total_minor += sale.total_minor;
    bucket.count += 1;
}

fn sales_in_range(
    sales: &[Sale],
    filter_of: impl Fn(&str) -> String,
    label_of: impl Fn(&str) -> String,
    from: &str,
    to: &str,
) -> BTreeMap<String, PeriodBucket> {
    let mut buckets = BTreeMap::new();
    for sale in sales {
        let key = filter_of(day_key(&sale.at));
        if key.as_str() < from || key.as_str() > to {
            continue;
        }
        bump_bucket(&mut buckets, label_of(day_key(&sale.at)), sale);
    }
    buckets
}

/// Daily totals across [fromISO, toISO], ascending; days without sales come back as zero buckets.
pub fn by_day(sales: &[Sale], from_iso: &str, to_iso: &str) -> Vec<PeriodBucket> {
    let from_key = day_key(from_iso).to_string();
    let to_key = day_key(to_iso).to_string();
    let by_label = sales_in_range(sales, str::to_string, str::to_string, &from_key, &to_key);
    let (Some(from_ms), Some(to_ms)) = (day_start_ms(&from_key), day_start_ms(&to_key)) else {
        return vec![];
    };
    let mut buckets = vec![];
    let mut ms = from_ms;
    while ms <= to_ms {
        let label = ms_day_key(ms);
        buckets.push(by_label.get(&label).cloned().unwrap_or(PeriodBucket { day: label, total_minor: 0, count: 0 }));
        ms += MS_DAY;
    }
    buckets
}

/// ISO-week totals across [fromISO, toISO] (labels like 2026-W39); empty weeks come back as zero buckets.
pub fn by_week(sales: &[Sale], from_iso: &str, to_iso: &str) -> Vec<PeriodBucket> {
    let from_key = day_key(from_iso).to_string();
    let to_key = day_key(to_iso).to_string();
    let by_label = sales_in_range(sales, str::to_string, iso_week_key, &from_key, &to_key);
    let (Some(start), Some(end)) = (day_start_ms(&from_key), day_start_ms(&to_key)) else {
        return vec![];
    };
    let dow0 = day_of_week(&from_key) as i64;
    let dow = if dow0 == 0 { 7 } else { dow0 };
    let mut buckets = vec![];
    let mut ms = start - (dow - 1) * MS_DAY;
    while ms <= end {
        let label = iso_week_key(&ms_day_key(ms));
        buckets.push(by_label.get(&label).cloned().unwrap_or(PeriodBucket { day: label, total_minor: 0, count: 0 }));
        ms += 7 * MS_DAY;
    }
    buckets
}

/// Monthly totals across [fromISO, toISO] (labels YYYY-MM); empty months come back as zero buckets.
pub fn by_month(sales: &[Sale], from_iso: &str, to_iso: &str) -> Vec<PeriodBucket> {
    let from_key = day_key(from_iso).get(..7).unwrap_or("").to_string();
    let to_key = day_key(to_iso).get(..7).unwrap_or("").to_string();
    let by_label = sales_in_range(sales, |k| k.get(..7).unwrap_or("").to_string(), |k| k.get(..7).unwrap_or("").to_string(), &from_key, &to_key);
    let Ok(mut year) = from_key.get(..4).unwrap_or("").parse::<i64>() else {
        return vec![];
    };
    let Ok(mut month) = from_key.get(5..7).unwrap_or("").parse::<i64>() else {
        return vec![];
    };
    let Ok(end_year) = to_key.get(..4).unwrap_or("").parse::<i64>() else {
        return vec![];
    };
    let Ok(end_month) = to_key.get(5..7).unwrap_or("").parse::<i64>() else {
        return vec![];
    };
    let mut buckets = vec![];
    while year < end_year || (year == end_year && month <= end_month) {
        let label = format!("{:04}-{:02}", year, month);
        buckets.push(by_label.get(&label).cloned().unwrap_or(PeriodBucket { day: label, total_minor: 0, count: 0 }));
        month += 1;
        if month > 12 {
            month = 1;
            year += 1;
        }
    }
    buckets
}
