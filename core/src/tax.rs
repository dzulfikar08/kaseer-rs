//! Tax computation. Rates are basis points (1_100 = 11%). Inclusive rates
//! (common in Indonesian retail: shelf price already contains PPN) extract
//! the tax portion; exclusive rates add on top. All rounding is exact
//! integer half-up: no float drift can move a receipt.

use crate::currency::{round_half_up_div, Currency};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaxRate {
    pub id: String,
    pub name: String,
    pub bps: i64,
    pub inclusive: bool,
}

pub const NO_TAX: &str = "none";

pub fn tax_on(base_minor: i64, rate: Option<&TaxRate>, _currency: &Currency) -> i64 {
    let Some(rate) = rate else { return 0 };
    if rate.id == NO_TAX || rate.bps == 0 {
        return 0;
    }
    if rate.inclusive {
        round_half_up_div(base_minor * rate.bps, 10_000 + rate.bps)
    } else {
        round_half_up_div(base_minor * rate.bps, 10_000)
    }
}

/// The taxable-or-not metadata a cart line carries (rate id).
pub fn resolve_rate<'a>(rate_id: Option<&str>, rates: &'a [TaxRate]) -> Option<&'a TaxRate> {
    let id = rate_id?;
    if id == NO_TAX {
        return None;
    }
    rates.iter().find(|r| r.id == id)
}
