//! Payment evaluation. Tenders apply sequentially against the remaining
//! balance: card/QRIS/transfer must not overpay, cash may (that is what
//! change is). Pure; the UI reads the returned state to decide whether the
//! complete button exists.

use std::sync::atomic::{AtomicI64, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TenderMethod {
    Cash,
    Card,
    Qris,
    Transfer,
    Loyalty,
}

impl TenderMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            TenderMethod::Cash => "cash",
            TenderMethod::Card => "card",
            TenderMethod::Qris => "qris",
            TenderMethod::Transfer => "transfer",
            TenderMethod::Loyalty => "loyalty",
        }
    }

    pub fn parse(s: &str) -> Option<TenderMethod> {
        match s {
            "cash" => Some(TenderMethod::Cash),
            "card" => Some(TenderMethod::Card),
            "qris" => Some(TenderMethod::Qris),
            "transfer" => Some(TenderMethod::Transfer),
            "loyalty" => Some(TenderMethod::Loyalty),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Tender {
    pub id: String,
    pub method: TenderMethod,
    /// Amount this tender wants to reduce the balance by.
    pub applied_minor: i64,
    /// Cash handed over; defaults to applied_minor for cash tenders.
    pub given_minor: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentState {
    pub remaining_minor: i64,
    pub change_minor: i64,
    pub complete: bool,
    pub invalid: Option<String>,
}

static TENDER_SEQ: AtomicI64 = AtomicI64::new(0);

pub fn new_tender(method: TenderMethod, applied_minor: i64, given_minor: Option<i64>) -> Tender {
    let seq = TENDER_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    Tender { id: format!("tender-{}", seq), method, applied_minor, given_minor }
}

pub fn evaluate_payments(total_minor: i64, tenders: &[Tender]) -> PaymentState {
    let mut remaining = total_minor;
    let mut change = 0i64;
    let mut invalid: Option<String> = None;

    for t in tenders {
        if t.applied_minor < 0 {
            invalid = Some(format!("Negative tender: {}", t.applied_minor));
            break;
        }
        let applied = t.applied_minor.min(remaining);
        if t.method == TenderMethod::Cash {
            let given = t.given_minor.unwrap_or(t.applied_minor);
            if given < applied {
                invalid = Some("Cash tender is short of its applied amount".to_string());
                break;
            }
            change += given - applied;
        } else if t.applied_minor > remaining {
            invalid = Some(format!("{} tender exceeds the remaining balance", t.method.as_str()));
            break;
        }
        remaining -= applied;
    }

    PaymentState {
        remaining_minor: remaining,
        change_minor: change.max(0),
        complete: remaining == 0 && invalid.is_none(),
        invalid,
    }
}
