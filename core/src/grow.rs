//! The register's half of the Grow loop: which add-on to offer with what
//! (the same rules Kaseer Cloud uses to value them, computed here from the
//! local ledger so the button works without the cloud), WhatsApp phone
//! numbers, and the WhatsApp receipt.

use std::collections::{HashMap, HashSet};

use crate::currency::Currency;
use crate::receipt::Sale;
use crate::repos::Product;

/// A pair must share at least this many baskets before it is offered.
const MIN_PAIR_BASKETS: i64 = 5;
/// Bought together at least 1.5x more often than chance (x100).
const MIN_LIFT_X100: i64 = 150;
/// And already together in at least this share of the anchor's baskets.
const MIN_ATTACH_PCT: i64 = 10;

/// "With `anchor_id`, offer `partner_id`."
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonPair {
    pub anchor_id: String,
    pub partner_id: String,
    /// Share of anchor baskets that already include the partner, percent.
    pub attach_pct: i64,
}

fn div_round(num: i64, den: i64) -> i64 {
    if den == 0 { 0 } else { (num * 2 + den) / (den * 2) }
}

/// Add-on pairs from recent sales (the caller passes the last 30 days),
/// keyed by anchor. The cheaper item is the add-on ("nasi goreng? add an
/// es teh"); on a price tie it goes with the more popular item. Archived
/// products are never offered, and each product is offered with at most
/// one anchor so a basket is never asked twice for the same thing.
pub fn addon_pairs(sales: &[Sale], products: &[Product]) -> HashMap<String, AddonPair> {
    let baskets = sales.len() as i64;
    if baskets == 0 {
        return HashMap::new();
    }
    let catalog: HashMap<&str, &Product> = products.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut singles: HashMap<&str, i64> = HashMap::new();
    let mut pairs: HashMap<(&str, &str), i64> = HashMap::new();
    for sale in sales {
        let mut ids: Vec<&str> = sale
            .lines
            .iter()
            .map(|l| l.product_id.as_str())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        ids.sort_unstable();
        for id in &ids {
            *singles.entry(id).or_insert(0) += 1;
        }
        for i in 0..ids.len() {
            for j in i + 1..ids.len() {
                *pairs.entry((ids[i], ids[j])).or_insert(0) += 1;
            }
        }
    }
    let price = |id: &str| catalog.get(id).map(|p| p.price_minor).unwrap_or(0);
    let mut rows: Vec<(i64, i64, AddonPair)> = vec![];
    for ((a, b), together) in pairs {
        if together < MIN_PAIR_BASKETS {
            continue;
        }
        let (Some(pa), Some(pb)) = (catalog.get(a), catalog.get(b)) else {
            continue;
        };
        if pa.archived || pb.archived {
            continue;
        }
        let sa = singles[a];
        let sb = singles[b];
        let lift_x100 = div_round(together * baskets * 100, sa * sb);
        if lift_x100 < MIN_LIFT_X100 {
            continue;
        }
        let (anchor, partner) =
            if price(a) > price(b) || (price(a) == price(b) && (sa > sb || (sa == sb && a < b))) { (a, b) } else { (b, a) };
        let anchor_baskets = singles[anchor];
        let anchor_only = anchor_baskets - together;
        if anchor_only <= 0 || price(partner) <= 0 || together * 100 < MIN_ATTACH_PCT * anchor_baskets {
            continue;
        }
        // Rank like the cloud's value: missed baskets times the add-on's price.
        rows.push((
            anchor_only * price(partner),
            lift_x100,
            AddonPair {
                anchor_id: anchor.to_string(),
                partner_id: partner.to_string(),
                attach_pct: div_round(together * 100, anchor_baskets),
            },
        ));
    }
    rows.sort_by(|x, y| {
        y.0.cmp(&x.0)
            .then(y.1.cmp(&x.1))
            .then_with(|| (&x.2.anchor_id, &x.2.partner_id).cmp(&(&y.2.anchor_id, &y.2.partner_id)))
    });
    let mut partners: HashSet<String> = HashSet::new();
    let mut out: HashMap<String, AddonPair> = HashMap::new();
    for (_, _, pair) in rows {
        if out.contains_key(&pair.anchor_id) || partners.contains(&pair.partner_id) {
            continue;
        }
        partners.insert(pair.partner_id.clone());
        out.insert(pair.anchor_id.clone(), pair);
    }
    out
}

/// Phone number in wa.me form (country code, digits only), or None when it
/// cannot be a WhatsApp number. Same rules as Kaseer Cloud: Indonesian
/// 08.., 8.., +62 (0)8.., 0062 8.. all become 628..; landlines are refused.
pub fn wa_phone(raw: &str, currency_code: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let international = t.starts_with('+') || t.starts_with("00");
    let mut n: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    if t.starts_with("00") {
        n = n[2..].to_string();
    }
    if international {
        if let Some(rest) = n.strip_prefix("620") {
            n = format!("62{rest}");
        }
    } else if currency_code == "IDR" {
        if let Some(rest) = n.strip_prefix('0') {
            n = format!("62{rest}");
        } else if n.starts_with('8') {
            n = format!("62{n}");
        } else if let Some(rest) = n.strip_prefix("620") {
            n = format!("62{rest}");
        }
    }
    if n.starts_with("62") && !n.starts_with("628") {
        return None;
    }
    if !international && currency_code == "IDR" && !n.starts_with("62") {
        return None;
    }
    (10..=15).contains(&n.len()).then_some(n)
}

/// A number's identity for "is this the same customer": the wa.me form when
/// it has one, otherwise its digits.
pub fn phone_key(raw: &str) -> String {
    wa_phone(raw, "IDR").unwrap_or_else(|| raw.chars().filter(|c| c.is_ascii_digit()).collect())
}

fn money(minor: i64, currency: &Currency) -> String {
    crate::money::format(&crate::money::money(minor, currency.code), Some(currency)).unwrap_or_else(|_| minor.to_string())
}

/// The receipt as a WhatsApp message.
pub fn receipt_text(sale: &Sale, store_name: &str, currency: &Currency) -> String {
    let id = currency.code == "IDR";
    let mut out = vec![format!("*{store_name}*"), sale.number.clone(), String::new()];
    for line in &sale.lines {
        let qty = if line.qty.fract() == 0.0 { format!("{}", line.qty as i64) } else { format!("{}", line.qty) };
        out.push(format!("{qty} × {}  {}", line.name, money(line.net_minor, currency)));
    }
    out.push(String::new());
    if sale.discount_minor > 0 {
        out.push(format!("{} -{}", if id { "Diskon" } else { "Discount" }, money(sale.discount_minor, currency)));
    }
    out.push(format!("*Total {}*", money(sale.total_minor, currency)));
    if sale.points_earned > 0 {
        out.push(if id { format!("+{} poin", sale.points_earned) } else { format!("+{} points", sale.points_earned) });
    }
    out.push(String::new());
    out.push(if id { "Terima kasih!".to_string() } else { "Thank you!".to_string() });
    out.join("\n")
}

/// `https://wa.me/<phone>?text=<message>`.
pub fn whatsapp_url(phone: &str, text: &str) -> String {
    let mut encoded = String::with_capacity(text.len() * 3);
    for b in text.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => encoded.push(*b as char),
            _ => encoded.push_str(&format!("%{b:02X}")),
        }
    }
    format!("https://wa.me/{phone}?text={encoded}")
}
