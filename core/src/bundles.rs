//! Smart bundles (Phase 3): co-purchase discovery over settled sales.
//! Support counts sales, not units: a pair scores 1 per sale whose lines
//! contain both products. Confidence is the pair's share of the more
//! popular partner's sales, kept in basis points so the API stays integer
//! end to end (10000 = every basket with that partner also had the other).

use std::collections::{HashMap, HashSet};

use crate::currency::round_half_up_div;
use crate::receipt::Sale;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundlePair {
    pub product_id_a: String,
    pub name_a: String,
    pub product_id_b: String,
    pub name_b: String,
    /// Sales containing both products.
    pub support: i64,
    /// support over the larger single-product sale count, in basis points (0..10000).
    pub confidence: i64,
}

/// Product pairs ordered by how often they share a basket, strongest first
/// (support desc, then confidence desc, then productId for stable ties).
/// A product renamed between sales keeps its first-seen name, so output is
/// a pure function of the input order.
pub fn co_purchase_pairs(sales: &[Sale], limit: usize) -> Vec<BundlePair> {
    let mut singles: HashMap<&str, i64> = HashMap::new();
    let mut first_names: HashMap<&str, &str> = HashMap::new();
    let mut pairs: HashMap<(&str, &str), i64> = HashMap::new();

    for sale in sales {
        let mut ids: HashSet<&str> = HashSet::new();
        for line in &sale.lines {
            ids.insert(&line.product_id);
            first_names.entry(&line.product_id).or_insert(&line.name);
        }
        for id in &ids {
            *singles.entry(id).or_insert(0) += 1;
        }
        // Sorted ids keep the pair key canonical no matter how the lines were
        // ordered on each receipt; productIdA < productIdB always holds here.
        let mut ordered: Vec<&str> = ids.iter().copied().collect();
        ordered.sort_unstable();
        let mut done: Vec<&str> = vec![];
        for product_id_b in ordered {
            for product_id_a in &done {
                *pairs.entry((product_id_a, product_id_b)).or_insert(0) += 1;
            }
            done.push(product_id_b);
        }
    }

    let mut rows: Vec<BundlePair> = pairs
        .into_iter()
        .map(|((product_id_a, product_id_b), support)| {
            let single_a = singles.get(product_id_a).copied().unwrap_or(0);
            let single_b = singles.get(product_id_b).copied().unwrap_or(0);
            let denominator = single_a.max(single_b);
            BundlePair {
                product_id_a: product_id_a.to_string(),
                name_a: first_names.get(product_id_a).copied().unwrap_or(product_id_a).to_string(),
                product_id_b: product_id_b.to_string(),
                name_b: first_names.get(product_id_b).copied().unwrap_or(product_id_b).to_string(),
                support,
                confidence: round_half_up_div(support * 10_000, denominator),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.support
            .cmp(&a.support)
            .then(b.confidence.cmp(&a.confidence))
            .then(a.product_id_a.cmp(&b.product_id_a))
            .then(a.product_id_b.cmp(&b.product_id_b))
    });
    rows.truncate(limit);
    rows
}

/// One "often bought with" row for the register: a partner product not in
/// the basket, the basket item it pairs with, and how many past sales held
/// both (the evidence shown to the cashier).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartnerSuggestion {
    pub product_id: String,
    pub anchor_id: String,
    pub anchor_name: String,
    pub support: i64,
}

/// Partners for the products in `basket`, strongest pair first (the input
/// order of [`co_purchase_pairs`]). A partner already in the basket is
/// skipped, each partner appears once, and pairs below `min_support` shared
/// sales are ignored: one shared basket is a coincidence, not a habit.
/// `available` filters partners the register can actually sell.
pub fn partner_suggestions(
    pairs: &[BundlePair],
    basket: &HashSet<&str>,
    min_support: i64,
    limit: usize,
    available: impl Fn(&str) -> bool,
) -> Vec<PartnerSuggestion> {
    let mut out: Vec<PartnerSuggestion> = vec![];
    for pair in pairs.iter().filter(|pair| pair.support >= min_support) {
        let sides = [
            (&pair.product_id_a, &pair.name_a, &pair.product_id_b),
            (&pair.product_id_b, &pair.name_b, &pair.product_id_a),
        ];
        for (anchor_id, anchor_name, partner) in sides {
            if basket.contains(anchor_id.as_str())
                && !basket.contains(partner.as_str())
                && !out.iter().any(|s| s.product_id == *partner)
                && available(partner)
            {
                out.push(PartnerSuggestion {
                    product_id: partner.clone(),
                    anchor_id: anchor_id.clone(),
                    anchor_name: anchor_name.clone(),
                    support: pair.support,
                });
            }
        }
        if out.len() >= limit {
            break;
        }
    }
    out.truncate(limit);
    out
}
