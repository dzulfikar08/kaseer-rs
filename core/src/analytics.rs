//! Dashboard analytics (Phase 2 BI): pure aggregates over settled sales.
//! Money is integer minor units throughout. Ranges arrive as caller-supplied
//! Sale batches so the clock never enters this module; the screen decides
//! what "today" means.

use std::collections::HashSet;

use crate::receipt::Sale;
use crate::repos::Customer;
use crate::reports::summarize;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DashboardStats {
    pub revenue_today_minor: i64,
    pub revenue_yesterday_minor: i64,
    pub revenue_this_month_minor: i64,
    /// Settled transactions today.
    pub tx_today: i64,
    /// Average basket today in minor units; 0 when today had no sales.
    pub avg_basket_today_minor: i64,
}

/// Header numbers for the owner's dashboard, from three sale batches.
pub fn dashboard_stats(today: &[Sale], yesterday: &[Sale], this_month: &[Sale]) -> DashboardStats {
    let t = summarize(today);
    DashboardStats {
        revenue_today_minor: t.total_minor,
        revenue_yesterday_minor: summarize(yesterday).total_minor,
        revenue_this_month_minor: summarize(this_month).total_minor,
        tx_today: t.count,
        avg_basket_today_minor: t.avg_basket_minor,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CustomerMix {
    /// Period buyers with no purchase before the period started (first-ever sale in period).
    pub new_customers: i64,
    /// Period buyers who had already purchased before the period started.
    pub returning_customers: i64,
}

/// New vs returning among today's (period's) buyers. A customer is new in
/// the period when the given sales history shows no purchase before the
/// period start; the period is the calendar day of `nowISO` (the app's
/// dayKey convention). Walk-in sales carry no customer and never count.
pub fn customer_mix(sales: &[Sale], customers: &[Customer], now_iso: &str) -> CustomerMix {
    let known: HashSet<&str> = customers.iter().map(|c| c.id.as_str()).collect();
    let period_start = format!("{}T00:00:00.000Z", now_iso.get(..10).unwrap_or(now_iso));
    let mut buyers: HashSet<&str> = HashSet::new();
    let mut prior_buyers: HashSet<&str> = HashSet::new();
    for sale in sales {
        let Some(customer_id) = sale.customer_id.as_deref() else { continue };
        if !known.contains(customer_id) {
            continue;
        }
        if sale.at.as_str() >= period_start.as_str() && sale.at.as_str() <= now_iso {
            buyers.insert(customer_id);
        } else if sale.at < period_start {
            prior_buyers.insert(customer_id);
        }
    }
    let mut mix = CustomerMix::default();
    for id in buyers {
        if prior_buyers.contains(id) {
            mix.returning_customers += 1;
        } else {
            mix.new_customers += 1;
        }
    }
    mix
}
