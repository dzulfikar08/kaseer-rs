//! Customer segments (Phase 4): pure classification over settled sales.
//! VISION.md definitions: new = first purchase within 30 days; loyal = five
//! or more visits, or a returning customer with three or more; lost = the
//! Phase 2 recency rule (no purchase within the configurable window);
//! high value = lifetime net in the top quartile of the customer base, or
//! four times the average basket when the base is too small to quartile.
//!
//! Segments overlap on purpose: a customer can be loyal and lost at once,
//! which is exactly the re-engagement case a campaign wants.

use std::collections::HashSet;

use crate::receipt::Sale;
use crate::repos::Customer;
use crate::reports::{parse_ms, MS_DAY};

const NEW_DAYS: i64 = 30;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Segments {
    pub new: Vec<String>,
    pub loyal: Vec<String>,
    pub lost: Vec<String>,
    pub high_value: Vec<String>,
}

struct CustomerStat {
    visits: i64,
    first_at_ms: i64,
    last_at_ms: i64,
    net_minor: i64,
}

pub fn segments(sales: &[Sale], customers: &[Customer], lost_after_days: i64, now_iso: &str) -> Segments {
    let now_ms = parse_ms(now_iso).unwrap_or(0);
    let mut stats: std::collections::HashMap<&str, CustomerStat> = std::collections::HashMap::new();
    for sale in sales {
        let Some(customer_id) = sale.customer_id.as_deref() else { continue };
        let at_ms = parse_ms(&sale.at).unwrap_or(0);
        match stats.get_mut(customer_id) {
            None => {
                stats.insert(customer_id, CustomerStat { visits: 1, first_at_ms: at_ms, last_at_ms: at_ms, net_minor: sale.net_minor });
            }
            Some(stat) => {
                stat.visits += 1;
                if at_ms < stat.first_at_ms {
                    stat.first_at_ms = at_ms;
                }
                if at_ms > stat.last_at_ms {
                    stat.last_at_ms = at_ms;
                }
                stat.net_minor += sale.net_minor;
            }
        }
    }

    let now_cutoff_new = now_ms - NEW_DAYS * MS_DAY;
    let lost_cutoff_ms = now_ms - lost_after_days * MS_DAY;
    let mut new_ids: Vec<String> = vec![];
    let mut loyal_ids: Vec<String> = vec![];
    let mut lost_ids: Vec<String> = vec![];
    let mut spenders: Vec<(&str, i64)> = vec![];

    for customer in customers {
        let Some(stat) = stats.get(customer.id.as_str()) else { continue };
        if stat.first_at_ms >= now_cutoff_new {
            new_ids.push(customer.id.clone());
        }
        // Loyal = 5+ visits, or a returning customer with 3+. Any customer
        // with 3+ visits is returning by definition, so both arms reduce
        // to the same integer test the TS oracle compiles down to.
        if stat.visits >= 3 {
            loyal_ids.push(customer.id.clone());
        }
        if stat.last_at_ms < lost_cutoff_ms {
            lost_ids.push(customer.id.clone());
        }
        if stat.net_minor > 0 {
            spenders.push((customer.id.as_str(), stat.net_minor));
        }
    }

    Segments {
        new: new_ids,
        loyal: loyal_ids,
        lost: lost_ids,
        high_value: high_value(customers, sales, spenders),
    }
}

/// High value = lifetime net in the top quartile. With four or more
/// customers the quartile is well defined (top ceil(n/4) by net, ties keep
/// customer order); below that the "top quarter" is one lucky regular, so
/// the rule falls back to four times the average basket. Integer
/// cross-multiplication keeps the comparison exact. Output keeps the
/// caller's customer order so tests and UI stay predictable.
fn high_value(customers: &[Customer], sales: &[Sale], spenders: Vec<(&str, i64)>) -> Vec<String> {
    if spenders.is_empty() {
        return vec![];
    }
    let mut picked: HashSet<&str> = HashSet::new();
    if customers.len() >= 4 {
        let take = customers.len().div_ceil(4);
        let mut ranked = spenders;
        ranked.sort_by_key(|(_, net)| std::cmp::Reverse(*net));
        for (id, _) in ranked.into_iter().take(take) {
            picked.insert(id);
        }
    } else {
        let total_net_minor: i64 = sales.iter().map(|s| s.net_minor).sum();
        let total_visits = sales.len() as i64;
        // net >= 4 x (totalNet / totalVisits), exact in integers.
        for (id, net_minor) in spenders {
            if total_visits > 0 && net_minor * total_visits >= 4 * total_net_minor {
                picked.insert(id);
            }
        }
    }
    customers
        .iter()
        .filter(|c| picked.contains(c.id.as_str()))
        .map(|c| c.id.clone())
        .collect()
}
