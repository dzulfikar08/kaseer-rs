//! Customer insights (Phase 2): visit counts, recency, lifetime value, and
//! the returning/lost split. Pure functions over the repo so the math stays
//! testable without a window. Definitions from VISION.md: returning = more
//! than one purchase; lost = no purchase within the configurable window.

use chrono::Utc;

use crate::receipt;
use crate::repos::{Repo, RepoError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerInsight {
    pub customer_id: String,
    pub visits: i64,
    pub last_visit_at: Option<String>,
    pub lifetime_net_minor: i64,
    pub returning: bool,
    pub lost: bool,
    pub days_since_last_visit: Option<i64>,
}

const MS_DAY: i64 = 24 * 60 * 60 * 1000;

/// One insight per customer, in the repo's (name-ordered) customer order.
pub fn insights_for(repo: &Repo, lost_after_days: i64) -> Result<Vec<CustomerInsight>, RepoError> {
    let now_ms = Utc::now().timestamp_millis();
    let lost_cutoff_ms = now_ms - lost_after_days * MS_DAY;
    let mut out = vec![];
    for customer in repo.customers(None)? {
        let sales: Vec<receipt::Sale> = repo.sales_by_customer(&customer.id)?;
        let mut last_visit_at: Option<String> = None;
        let mut lifetime_net_minor: i64 = 0;
        for s in &sales {
            lifetime_net_minor += s.net_minor;
            if last_visit_at.as_ref().is_none_or(|seen| s.at > *seen) {
                last_visit_at = Some(s.at.clone());
            }
        }
        let last_visit_ms = last_visit_at.as_deref().and_then(crate::reports::parse_ms);
        out.push(CustomerInsight {
            customer_id: customer.id,
            visits: sales.len() as i64,
            last_visit_at,
            lifetime_net_minor,
            returning: sales.len() > 1,
            lost: last_visit_ms.is_some_and(|ms| ms < lost_cutoff_ms),
            days_since_last_visit: last_visit_ms.map(|ms| ((now_ms - ms) / MS_DAY).max(0)),
        });
    }
    Ok(out)
}
