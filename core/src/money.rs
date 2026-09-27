//! Money: an integer minor-unit amount plus a currency code.
//!
//! Invariants enforced here:
//! - amounts are integers in minor units; floats never cross this API
//! - every operation checks currency compatibility and errors on mismatch
//! - formatting and parsing are inverse operations for canonical amounts

use crate::currency::{currency_by_code, Currency};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MoneyError {
    #[error("currency mismatch: {0} vs {1}")]
    CurrencyMismatch(String, String),
    #[error("unknown currency: {0}")]
    UnknownCurrency(String),
    #[error("negative amounts are not parseable: {0}")]
    NegativeAmount(String),
    #[error("unparseable amount: {0}")]
    Unparseable(String),
    #[error("{code} has no minor unit: {input}")]
    NoMinorUnit { code: String, input: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Money {
    pub minor: i64,
    pub currency: String,
}

pub fn money(minor: i64, currency: impl Into<String>) -> Money {
    Money { minor, currency: currency.into() }
}

fn assert_same(a: &Money, b: &Money) -> Result<(), MoneyError> {
    if a.currency != b.currency {
        return Err(MoneyError::CurrencyMismatch(a.currency.clone(), b.currency.clone()));
    }
    Ok(())
}

pub fn add(a: &Money, b: &Money) -> Result<Money, MoneyError> {
    assert_same(a, b)?;
    Ok(Money { minor: a.minor + b.minor, currency: a.currency.clone() })
}

pub fn sub(a: &Money, b: &Money) -> Result<Money, MoneyError> {
    assert_same(a, b)?;
    Ok(Money { minor: a.minor - b.minor, currency: a.currency.clone() })
}

pub fn eq(a: &Money, b: &Money) -> Result<bool, MoneyError> {
    assert_same(a, b)?;
    Ok(a.minor == b.minor)
}

/// Split `m` into parts proportional to `weights`, each an integer minor
/// amount, summing exactly to `m` (largest-remainder method). This is how
/// an order-level discount lands on individual lines without losing a
/// unit. Output length always equals `weights.len()`.
pub fn allocate(m: &Money, weights: &[i64]) -> Vec<Money> {
    let n = weights.len();
    if n == 0 {
        return vec![];
    }
    let total: i64 = weights.iter().sum();
    if total <= 0 {
        // Degenerate weights: split evenly, remainder on the first part.
        let even = m.minor / n as i64;
        let mut parts = vec![even; n];
        parts[0] += m.minor - even * n as i64;
        return parts.into_iter().map(|minor| money(minor, m.currency.clone())).collect();
    }
    // Exact integer largest-remainder: value floors num/total, frac keeps
    // the lost numerator so no float ever enters the split.
    struct Part {
        value: i64,
        frac: i64,
    }
    let mut parts: Vec<Part> = weights
        .iter()
        .map(|&w| {
            let num = m.minor * w;
            Part { value: num.div_euclid(total), frac: num.rem_euclid(total) }
        })
        .collect();
    let mut remainder = m.minor - parts.iter().map(|p| p.value).sum::<i64>();
    // Stable sort by frac descending (mirrors JS sort-stable semantics).
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| parts[b].frac.cmp(&parts[a].frac));
    for &i in &order {
        if remainder <= 0 {
            break;
        }
        parts[i].value += 1;
        remainder -= 1;
    }
    parts.into_iter().map(|p| money(p.value, m.currency.clone())).collect()
}

fn group_digits(digits: &str, sep: char) -> String {
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(*b as char);
    }
    out
}

/// Canonical display string: symbol, grouping, decimals. Never truncated.
pub fn format(m: &Money, currency: Option<&Currency>) -> Result<String, MoneyError> {
    let cur = match currency {
        Some(c) => c,
        None => currency_by_code(&m.currency)?,
    };
    if m.currency != cur.code {
        return Err(MoneyError::CurrencyMismatch(m.currency.clone(), cur.code.to_string()));
    }

    let negative = m.minor < 0;
    let abs = m.minor.unsigned_abs();
    let major = abs / cur.minor_per_major as u64;
    let minor_part = abs - major * cur.minor_per_major as u64;

    let major_str = group_digits(&major.to_string(), cur.thousands);
    let minor_str = if cur.decimals > 0 {
        format!("{}{:0>width$}", cur.decimal, minor_part, width = cur.decimals)
    } else {
        String::new()
    };

    let body = format!("{}{}", major_str, minor_str);
    if cur.symbol_after {
        Ok(if negative { format!("-{} {}", body, cur.symbol) } else { format!("{} {}", body, cur.symbol) })
    } else {
        let unsigned = if cur.symbol_space {
            format!("{} {}", cur.symbol, body)
        } else {
            format!("{}{}", cur.symbol, body)
        };
        Ok(if negative { format!("-{}", unsigned) } else { unsigned })
    }
}

/// Parse a typed or scanned amount string ("12.500" in IDR, "1,234.50" in
/// USD) into minor units. Lenient about spacing and the symbol, strict
/// about digits. Errors on anything it cannot interpret exactly.
pub fn parse(input: &str, currency: &Currency) -> Result<Money, MoneyError> {
    let s = input.trim().replacen(currency.symbol, "", 1);
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if s.starts_with('-') {
        return Err(MoneyError::NegativeAmount(input.to_string()));
    }

    // Drop thousands separators, normalize the decimal separator.
    let s = s.replace(currency.thousands, "");
    let s = if currency.decimal != '.' { s.replace(currency.decimal, ".") } else { s };

    // ^\d+(\.\d+)?$
    let valid = {
        let mut it = s.split('.');
        let int_part = it.next().unwrap_or("");
        let frac = it.next();
        it.next().is_none()
            && !int_part.is_empty()
            && int_part.bytes().all(|b| b.is_ascii_digit())
            && match frac {
                None => true,
                Some(f) => !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()),
            }
    };
    if !valid {
        return Err(MoneyError::Unparseable(input.to_string()));
    }

    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s.as_str(), ""),
    };
    let int: i64 = int_part
        .parse()
        .map_err(|_| MoneyError::Unparseable(input.to_string()))?;

    if currency.minor_per_major == 1 {
        if !frac_part.is_empty() {
            return Err(MoneyError::NoMinorUnit {
                code: currency.code.to_string(),
                input: input.to_string(),
            });
        }
        return Ok(money(int, currency.code));
    }

    if frac_part.len() > currency.decimals {
        // More fractional digits than the currency supports: round half-up.
        let value: f64 = format!("{}.{}", int_part, frac_part)
            .parse()
            .map_err(|_| MoneyError::Unparseable(input.to_string()))?;
        let rounded = (value * currency.minor_per_major as f64).round() as i64;
        return Ok(money(rounded, currency.code));
    }

    let padded = format!("{:0<width$}", frac_part, width = currency.decimals);
    let minor = int * currency.minor_per_major + padded.parse::<i64>().unwrap_or(0);
    Ok(money(minor, currency.code))
}
