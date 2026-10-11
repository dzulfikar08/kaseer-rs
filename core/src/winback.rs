//! Win-back (Phase 3a): find customers who went quiet and reach them before
//! they are gone for good. Pure functions over sale/customer records so the
//! math stays testable without a window. Definitions: lost = no purchase
//! within the configurable window (same boundary as customer-insights);
//! frequency dropped = the last-30d visit rate fell below half of the 30-90d
//! rate, only judged for customers with 4+ lifetime visits so the comparison
//! has signal.

use crate::receipt::Sale;
use crate::repos::Customer;
use crate::reports::{ms_to_iso, parse_ms, MS_DAY};

/// Recent window: the last 30 days before now.
const RECENT_DAYS: i64 = 30;
/// Prior window: the 60 days before that (30-90 days ago).
const PRIOR_DAYS: i64 = 60;
/// Minimum lifetime visits before a rate comparison means anything.
const MIN_VISITS_FOR_TREND: i64 = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct WinbackEntry {
    pub customer_id: String,
    pub name: String,
    pub phone: String,
    pub last_visit_at: String,
    pub days_since: i64,
    pub visits: i64,
    pub previous_visits_per_week: f64,
    pub recent_visits_per_week: f64,
    pub frequency_dropped: bool,
}

struct VisitStat {
    visits: i64,
    last_visit_ms: i64,
    recent: i64,
    prior: i64,
}

/// Lost customers, longest silent first (ties by name). A customer is listed
/// when they have at least one visit and their last visit is strictly older
/// than `lost_after_days`; exactly on the cutoff they are not lost yet.
pub fn winback_list(sales: &[Sale], customers: &[Customer], lost_after_days: i64, now_iso: &str) -> Vec<WinbackEntry> {
    let now_ms = parse_ms(now_iso).unwrap_or(0);
    let lost_cutoff_ms = now_ms - lost_after_days * MS_DAY;
    let recent_start_ms = now_ms - RECENT_DAYS * MS_DAY;
    let prior_start_ms = now_ms - (RECENT_DAYS + PRIOR_DAYS) * MS_DAY;

    let mut stats: std::collections::HashMap<&str, VisitStat> = std::collections::HashMap::new();
    for sale in sales {
        let Some(customer_id) = sale.customer_id.as_deref() else { continue };
        let at_ms = parse_ms(&sale.at).unwrap_or(0);
        let s = stats.entry(customer_id).or_insert(VisitStat {
            visits: 0,
            last_visit_ms: at_ms,
            recent: 0,
            prior: 0,
        });
        s.visits += 1;
        if at_ms > s.last_visit_ms {
            s.last_visit_ms = at_ms;
        }
        if at_ms >= recent_start_ms {
            s.recent += 1;
        } else if at_ms >= prior_start_ms {
            s.prior += 1;
        }
    }

    let mut entries: Vec<(i64, WinbackEntry)> = vec![];
    for customer in customers {
        let Some(s) = stats.get(customer.id.as_str()) else { continue };
        if s.visits == 0 || s.last_visit_ms >= lost_cutoff_ms {
            continue;
        }
        let recent_rate = s.recent as f64 / (RECENT_DAYS as f64 / 7.0);
        let prior_rate = s.prior as f64 / (PRIOR_DAYS as f64 / 7.0);
        // Rates keep two decimals in the exported copy; the drop verdict
        // compares the raw rates, so rounding never flips it.
        entries.push((
            s.last_visit_ms,
            WinbackEntry {
                customer_id: customer.id.clone(),
                name: customer.name.clone(),
                phone: customer.phone.clone(),
                last_visit_at: ms_to_iso(s.last_visit_ms),
                days_since: ((now_ms - s.last_visit_ms) / MS_DAY).max(0),
                visits: s.visits,
                previous_visits_per_week: (prior_rate * 100.0).round() / 100.0,
                recent_visits_per_week: (recent_rate * 100.0).round() / 100.0,
                frequency_dropped: s.visits >= MIN_VISITS_FOR_TREND && recent_rate < prior_rate / 2.0,
            },
        ));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.cmp(&b.1.name)));
    entries.into_iter().map(|(_, entry)| entry).collect()
}

/// What the register says about an attached customer: lifetime visits,
/// whole days since the last one, and whether that gap crosses the lost
/// boundary (the same strict "older than lost_after_days" test as
/// [`winback_list`], so a customer the win-back list calls lost is greeted
/// as coming back).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recognition {
    pub visits: i64,
    /// `None` for a customer with no settled sale yet.
    pub days_since: Option<i64>,
    pub returning_after_lost: bool,
}

pub fn recognition(visits: i64, last_visit_at: Option<&str>, lost_after_days: i64, now_iso: &str) -> Recognition {
    let now_ms = parse_ms(now_iso).unwrap_or(0);
    let last_ms = last_visit_at.and_then(parse_ms).filter(|_| visits > 0);
    Recognition {
        visits,
        days_since: last_ms.map(|ms| ((now_ms - ms) / MS_DAY).max(0)),
        returning_after_lost: last_ms.is_some_and(|ms| ms < now_ms - lost_after_days * MS_DAY),
    }
}

/// 'name;phone' lines for the contacts a WhatsApp blast can actually reach.
pub fn whatsapp_ready(list: &[WinbackEntry]) -> String {
    let lines: Vec<String> = list
        .iter()
        .filter(|e| !e.phone.trim().is_empty())
        .map(|e| format!("{};{}", e.name, e.phone))
        .collect();
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

/// Quote a CSV field only when its raw form would break the row (RFC 4180).
fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Re-engagement sheet: one row per lost customer with the trend columns.
pub fn reengagement_csv(list: &[WinbackEntry]) -> String {
    let header = "name,phone,last_visit_at,days_since,visits,previous_visits_per_week,recent_visits_per_week,frequency_dropped\n";
    let body = list
        .iter()
        .map(|e| {
            [
                csv_field(&e.name),
                csv_field(&e.phone),
                e.last_visit_at.clone(),
                e.days_since.to_string(),
                e.visits.to_string(),
                format_rate(e.previous_visits_per_week),
                format_rate(e.recent_visits_per_week),
                if e.frequency_dropped { "true".into() } else { "false".into() },
            ]
            .join(",")
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}{}", header, if body.is_empty() { String::new() } else { format!("{}\n", body) })
}

/// Plain-decimal rate text, matching JS String(number): 0 -> "0", 0.7 -> "0.7".
fn format_rate(rate: f64) -> String {
    format!("{}", rate)
}
