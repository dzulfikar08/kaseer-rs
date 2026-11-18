//! Profit guard: how far a whole-sale discount can go before the sale
//! earns less than the owner's minimum margin over HPP (cost of goods).
//!
//! Only lines whose product has a cost (HPP) count: a product without one
//! is unknown, not free. The order discount is spread over the lines in
//! proportion to their amount, so the known-cost lines alone decide the
//! limit. Revenue is the line's net without inclusive tax (tax is the
//! government's money, not the store's).

use std::collections::HashMap;

use crate::cart::{compute_totals, set_order_discount, Cart, Discount};
use crate::currency::Currency;
use crate::tax::{resolve_rate, TaxRate};

/// Revenue and HPP of the lines that have a cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CartProfit {
    pub revenue_minor: i64,
    pub cost_minor: i64,
}

impl CartProfit {
    pub fn profit_minor(&self) -> i64 {
        self.revenue_minor - self.cost_minor
    }

    /// Profit as a share of revenue, percent rounded down; None on zero revenue.
    pub fn margin_pct(&self) -> Option<i64> {
        (self.revenue_minor > 0).then(|| (self.profit_minor() * 100).div_euclid(self.revenue_minor))
    }

    /// At least `min_margin_pct` of the revenue is profit.
    pub fn meets(&self, min_margin_pct: i64) -> bool {
        self.profit_minor() * 100 >= min_margin_pct * self.revenue_minor
    }
}

/// The cart's profit over its known-cost lines, as it stands (with its
/// discounts). None when no line has a cost.
pub fn cart_profit(cart: &Cart, costs: &HashMap<String, i64>, rates: &[TaxRate], currency: &Currency) -> Option<CartProfit> {
    let totals = compute_totals(cart, rates, currency);
    let mut out = CartProfit { revenue_minor: 0, cost_minor: 0 };
    let mut any = false;
    for (line, t) in cart.lines.iter().zip(&totals.lines) {
        let Some(&cost) = costs.get(&line.product_id).filter(|c| **c > 0) else {
            continue;
        };
        any = true;
        let inclusive = resolve_rate(line.tax_rate_id.as_deref(), rates).is_some_and(|r| r.inclusive);
        out.revenue_minor += if inclusive { t.net_minor - t.tax_minor } else { t.net_minor };
        out.cost_minor += (cost as f64 * line.qty).round() as i64;
    }
    any.then_some(out)
}

/// The largest whole-sale percent discount (basis points) that keeps the
/// sale at or above `min_margin_pct`. None when no line has a cost (no
/// guard); Some(0) when even the full price is under the floor.
pub fn max_order_discount_bps(
    cart: &Cart,
    costs: &HashMap<String, i64>,
    rates: &[TaxRate],
    currency: &Currency,
    min_margin_pct: i64,
) -> Option<i64> {
    let base = set_order_discount(cart, None);
    let full = cart_profit(&base, costs, rates, currency)?;
    let keep = (100 - min_margin_pct.clamp(0, 99)) as f64 / 100.0;
    if full.revenue_minor <= 0 || !full.meets(min_margin_pct) {
        return Some(0);
    }
    // (1 - d) * revenue * keep >= cost  =>  d <= 1 - cost / (revenue * keep)
    let d = 1.0 - full.cost_minor as f64 / (full.revenue_minor as f64 * keep);
    let mut bps = ((d * 10_000.0).floor() as i64).clamp(0, 10_000);
    // Minor-unit rounding can tip the exact limit: step down until it holds.
    let holds = |bps: i64| {
        let c = set_order_discount(&base, Some(Discount::Percent { bps }));
        cart_profit(&c, costs, rates, currency).is_none_or(|p| p.meets(min_margin_pct))
    };
    while bps > 0 && !holds(bps) {
        bps -= 1;
    }
    Some(bps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cart::{add_to_cart, empty_cart, AddableProduct};
    use crate::currency::currency_by_code;

    fn product(id: &str, price: i64) -> AddableProduct {
        AddableProduct { id: id.into(), name: id.into(), sku: id.into(), price_minor: price, tax_rate_id: None }
    }

    fn idr() -> &'static Currency {
        currency_by_code("IDR").unwrap()
    }

    #[test]
    fn no_cost_means_no_guard() {
        let cart = add_to_cart(&empty_cart("IDR"), &product("kopi", 18_000), 1.0).unwrap();
        assert_eq!(max_order_discount_bps(&cart, &HashMap::new(), &[], idr(), 10), None);
    }

    #[test]
    fn limit_keeps_the_minimum_margin() {
        // Rp 18.000 with HPP Rp 6.500, keep 10%: 1 - 6500 / (18000 * 0.9) = 59.87%.
        let cart = add_to_cart(&empty_cart("IDR"), &product("kopi", 18_000), 1.0).unwrap();
        let costs = HashMap::from([("kopi".to_string(), 6_500)]);
        let bps = max_order_discount_bps(&cart, &costs, &[], idr(), 10).unwrap();
        assert_eq!(bps, 5_987);
        let at = set_order_discount(&cart, Some(Discount::Percent { bps }));
        assert!(cart_profit(&at, &costs, &[], idr()).unwrap().meets(10));
        let over = set_order_discount(&cart, Some(Discount::Percent { bps: bps + 10 }));
        assert!(!cart_profit(&over, &costs, &[], idr()).unwrap().meets(10));
    }

    #[test]
    fn already_under_the_floor_allows_nothing() {
        // Groceries: Rp 3.500 sold, Rp 2.800 HPP is 20%; a 25% floor is out of reach.
        let cart = add_to_cart(&empty_cart("IDR"), &product("mie", 3_500), 1.0).unwrap();
        let costs = HashMap::from([("mie".to_string(), 2_800)]);
        assert_eq!(max_order_discount_bps(&cart, &costs, &[], idr(), 25), Some(0));
    }

    #[test]
    fn unknown_cost_lines_do_not_count() {
        let cart = add_to_cart(&empty_cart("IDR"), &product("kopi", 18_000), 1.0).unwrap();
        let cart = add_to_cart(&cart, &product("mystery", 50_000), 1.0).unwrap();
        let costs = HashMap::from([("kopi".to_string(), 6_500)]);
        assert_eq!(max_order_discount_bps(&cart, &costs, &[], idr(), 10), Some(5_987));
    }

    #[test]
    fn inclusive_tax_is_not_revenue() {
        let rates = [TaxRate { id: "ppn".into(), name: "PPN".into(), bps: 1_100, inclusive: true }];
        let mut p = product("kopi", 11_100);
        p.tax_rate_id = Some("ppn".into());
        let cart = add_to_cart(&empty_cart("IDR"), &p, 1.0).unwrap();
        let costs = HashMap::from([("kopi".to_string(), 5_000)]);
        let profit = cart_profit(&cart, &costs, &rates, idr()).unwrap();
        assert_eq!(profit.revenue_minor, 10_000);
        assert_eq!(profit.margin_pct(), Some(50));
    }
}
