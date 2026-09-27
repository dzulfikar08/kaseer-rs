//! Currency registry. A currency is data, not code: every money format and
//! parse in the app derives from these definitions, so adding a market is
//! a registry entry, not a refactor.

use crate::money::MoneyError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Currency {
    /// ISO 4217 code.
    pub code: &'static str,
    /// How many minor units make one major unit (IDR: 1, USD: 100).
    pub minor_per_major: i64,
    /// Digits printed after the decimal separator.
    pub decimals: usize,
    pub symbol: &'static str,
    /// True when the symbol trails the amount ("12,50 €").
    pub symbol_after: bool,
    /// True when a space separates symbol and digits ("Rp 12.500", not "$1,234").
    pub symbol_space: bool,
    pub thousands: char,
    pub decimal: char,
    /// Banknotes offered as quick-tender chips, in minor units.
    pub cash_notes: &'static [i64],
}

const IDR_NOTES: &[i64] = &[2_000, 5_000, 10_000, 20_000, 50_000, 100_000];
const USD_NOTES: &[i64] = &[100, 500, 1_000, 2_000, 5_000, 10_000];

pub const IDR: Currency = Currency {
    code: "IDR",
    minor_per_major: 1,
    decimals: 0,
    symbol: "Rp",
    symbol_after: false,
    symbol_space: true,
    thousands: '.',
    decimal: ',',
    cash_notes: IDR_NOTES,
};

pub const USD: Currency = Currency {
    code: "USD",
    minor_per_major: 100,
    decimals: 2,
    symbol: "$",
    symbol_after: false,
    symbol_space: false,
    thousands: ',',
    decimal: '.',
    cash_notes: USD_NOTES,
};

pub const EUR: Currency = Currency {
    code: "EUR",
    symbol: "€",
    symbol_after: true,
    symbol_space: true,
    ..USD
};

pub const GBP: Currency = Currency { code: "GBP", symbol: "£", ..USD };
pub const SGD: Currency = Currency { code: "SGD", symbol: "S$", ..USD };
pub const AUD: Currency = Currency { code: "AUD", symbol: "A$", ..USD };
pub const MYR: Currency = Currency {
    code: "MYR",
    minor_per_major: 100,
    decimals: 2,
    symbol: "RM",
    symbol_after: false,
    symbol_space: true,
    thousands: ',',
    decimal: '.',
    cash_notes: USD_NOTES,
};
pub const JPY: Currency = Currency {
    code: "JPY",
    minor_per_major: 1,
    decimals: 0,
    symbol: "¥",
    ..USD
};
pub const CNY: Currency = Currency { code: "CNY", symbol: "¥", ..USD };

pub const CURRENCIES: &[Currency] = &[IDR, USD, EUR, GBP, SGD, AUD, MYR, JPY, CNY];

/// Exact integer half-up division: round(num/den) without float drift.
/// num must be a non-negative integer of safe magnitude (all call sites
/// are base*bps or unit*qtyScaled products).
pub fn round_half_up_div(num: i64, den: i64) -> i64 {
    (2 * num + den) / (2 * den)
}

/// Half-up rounding to the currency's minor precision.
pub fn round_to_minor(value: f64, currency: &Currency) -> i64 {
    let scaled = value * currency.minor_per_major as f64;
    (scaled.abs().round() * scaled.signum()) as i64
}

pub fn currency_by_code(code: &str) -> Result<&'static Currency, MoneyError> {
    CURRENCIES
        .iter()
        .find(|c| c.code == code)
        .ok_or_else(|| MoneyError::UnknownCurrency(code.to_string()))
}

/// Convert a minor-unit amount to its major-unit decimal (display only).
pub fn minor_to_major(minor: i64, currency: &Currency) -> f64 {
    minor as f64 / currency.minor_per_major as f64
}
