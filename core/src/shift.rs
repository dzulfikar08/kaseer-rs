//! Shift lifecycle and drawer math. A shift gates selling: the register
//! refuses carts while no shift is open. Cash expectation is derived from
//! the ledger (opening float, cash applied, change handed back), never
//! from a running mutable total.

use crate::payments::TenderMethod;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shift {
    pub id: String,
    pub opened_at: String,
    pub opening_float_minor: i64,
    pub currency: String,
    pub closed_at: Option<String>,
    pub counted_minor: Option<i64>,
}

/// One settled sale, reduced to what drawer and report math need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenderLine {
    pub method: TenderMethod,
    pub applied_minor: i64,
    pub change_minor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettledSale {
    pub gross_minor: i64,
    pub discount_minor: i64,
    pub tax_minor: i64,
    pub total_minor: i64,
    pub tenders: Vec<TenderLine>,
}

pub fn cash_expected_minor(shift: &Shift, sales: &[SettledSale]) -> i64 {
    let mut cash = shift.opening_float_minor;
    for sale in sales {
        for t in &sale.tenders {
            if t.method == TenderMethod::Cash {
                cash += t.applied_minor;
                cash -= t.change_minor;
            }
        }
    }
    cash
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ByMethod {
    pub cash: i64,
    pub card: i64,
    pub qris: i64,
    pub transfer: i64,
    pub loyalty: i64,
}

impl ByMethod {
    pub fn record(&mut self, method: TenderMethod, minor: i64) {
        match method {
            TenderMethod::Cash => self.cash += minor,
            TenderMethod::Card => self.card += minor,
            TenderMethod::Qris => self.qris += minor,
            TenderMethod::Transfer => self.transfer += minor,
            TenderMethod::Loyalty => self.loyalty += minor,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShiftTotals {
    pub sales_count: usize,
    pub gross_minor: i64,
    pub discount_minor: i64,
    pub tax_minor: i64,
    pub total_minor: i64,
    pub by_method: ByMethod,
}

pub fn shift_totals(sales: &[SettledSale]) -> ShiftTotals {
    let mut by_method = ByMethod::default();
    for sale in sales {
        for t in &sale.tenders {
            by_method.record(t.method, t.applied_minor);
        }
    }
    ShiftTotals {
        sales_count: sales.len(),
        gross_minor: sales.iter().map(|x| x.gross_minor).sum(),
        discount_minor: sales.iter().map(|x| x.discount_minor).sum(),
        tax_minor: sales.iter().map(|x| x.tax_minor).sum(),
        total_minor: sales.iter().map(|x| x.total_minor).sum(),
        by_method,
    }
}

pub fn count_variance_minor(expected_minor: i64, counted_minor: i64) -> i64 {
    counted_minor - expected_minor
}

/// One row of a physical drawer count: how many pieces of one denomination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenominationCount {
    pub minor: i64,
    pub count: i64,
}

/// Sum of a drawer count in minor units. An empty count is zero cash.
pub fn count_to_minor(counts: &[DenominationCount]) -> i64 {
    counts.iter().map(|c| c.minor * c.count).sum()
}
