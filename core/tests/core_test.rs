//! Ported from kaseer/src/core/core.test.ts — the money-safety proof for
//! the core domain. Every expected number was computed by hand in the TS
//! oracle; the register's arithmetic must match it exactly. The `reports`
//! block of the oracle is excluded: reports.ts is a later crew's scope.

use std::collections::BTreeMap;

use kaseer_core::cart::{
    add_to_cart, compute_totals, empty_cart, set_line_qty, set_order_discount, AddableProduct,
    Cart, Discount,
};
use kaseer_core::currency::{self, currency_by_code, round_half_up_div, CURRENCIES};
use kaseer_core::inventory::{is_low_stock, stock_on_hand, StockMove, StockReason};
use kaseer_core::loyalty::{max_redeemable_points, points_earned, LoyaltyConfig};
use kaseer_core::money::{allocate, format, parse};
use kaseer_core::payments::{evaluate_payments, new_tender, TenderMethod};
use kaseer_core::receipt::{render_receipt_text, sale_number, Sale, SaleLine, SaleTender, StoreInfo};
use kaseer_core::shift::{cash_expected_minor, count_variance_minor, shift_totals, SettledSale, Shift, TenderLine};
use kaseer_core::tax::{tax_on, TaxRate};

// ── shared fixtures ──────────────────────────────────────────────────────

fn format_m(minor: i64, cur: &currency::Currency) -> String {
    format(&kaseer_core::money::money(minor, cur.code), Some(cur)).unwrap()
}

fn allocate_m(minor: i64, weights: &[i64]) -> Vec<i64> {
    allocate(&kaseer_core::money::money(minor, "IDR"), weights)
        .iter()
        .map(|p| p.minor)
        .collect()
}

fn tax(base_minor: i64, rate: Option<&TaxRate>) -> i64 {
    tax_on(base_minor, rate, &currency::IDR)
}

fn ppn11() -> TaxRate {
    TaxRate { id: "ppn11".into(), name: "PPN 11%".into(), bps: 1_100, inclusive: true }
}

fn ppn11_exc() -> TaxRate {
    TaxRate { id: "ppn11x".into(), name: "PPN 11% added".into(), bps: 1_100, inclusive: false }
}

fn p1() -> AddableProduct {
    AddableProduct {
        id: "p1".into(),
        name: "Kopi".into(),
        sku: "111".into(),
        price_minor: 18_000,
        tax_rate_id: Some("ppn11".into()),
    }
}

fn p2() -> AddableProduct {
    AddableProduct {
        id: "p2".into(),
        name: "Beras".into(),
        sku: "222".into(),
        price_minor: 68_000,
        tax_rate_id: None,
    }
}

fn cart_with_p1_p2() -> Cart {
    let cart = empty_cart("IDR");
    let cart = add_to_cart(&cart, &p1(), 2.0).unwrap();
    add_to_cart(&cart, &p2(), 1.0).unwrap()
}

fn fixture_shift() -> Shift {
    Shift {
        id: "s1".into(),
        opened_at: "2026-09-26T09:00:00Z".into(),
        opening_float_minor: 500_000,
        currency: "IDR".into(),
        closed_at: None,
        counted_minor: None,
    }
}

fn sale_cash(cash_applied: i64, change: i64) -> SettledSale {
    SettledSale {
        gross_minor: cash_applied,
        discount_minor: 0,
        tax_minor: 0,
        total_minor: cash_applied,
        tenders: vec![TenderLine {
            method: TenderMethod::Cash,
            applied_minor: cash_applied,
            change_minor: change,
        }],
    }
}

fn fixture_sale() -> Sale {
    let mut tax_by_rate_id = BTreeMap::new();
    tax_by_rate_id.insert("ppn11".to_string(), 3_568);
    Sale {
        id: "sale-1".into(),
        number: "KSR-20260926-0001".into(),
        at: "2026-09-26T17:27:00.000Z".into(),
        currency: "IDR".into(),
        shift_id: Some("s1".into()),
        customer_id: None,
        customer_name_snapshot: None,
        lines: vec![
            SaleLine {
                product_id: "p1".into(),
                name: "Kopi Susu Gula Aren".into(),
                sku: "111".into(),
                qty: 2.0,
                unit_minor: 18_000,
                cost_minor: None,
                discount_minor: 0,
                net_minor: 36_000,
                tax_minor: 3_568,
                tax_rate_id: Some("ppn11".into()),
            },
            SaleLine {
                product_id: "p2".into(),
                name: "Beras Premium 5kg".into(),
                sku: "222".into(),
                qty: 1.0,
                unit_minor: 68_000,
                cost_minor: None,
                discount_minor: 0,
                net_minor: 68_000,
                tax_minor: 0,
                tax_rate_id: None,
            },
        ],
        tenders: vec![SaleTender {
            method: TenderMethod::Cash,
            applied_minor: 104_000,
            given_minor: Some(110_000),
        }],
        gross_minor: 104_000,
        discount_minor: 0,
        net_minor: 104_000,
        tax_minor: 3_568,
        total_minor: 104_000,
        change_minor: 6_000,
        points_earned: 10,
        points_redeemed: 0,
        tax_by_rate_id,
    }
}

// ── money format ─────────────────────────────────────────────────────────

#[test]
fn idr_groups_with_dots_and_no_decimals() {
    assert_eq!(format_m(12_500, &currency::IDR), "Rp 12.500");
    assert_eq!(format_m(1_000_000, &currency::IDR), "Rp 1.000.000");
    assert_eq!(format_m(-500, &currency::IDR), "-Rp 500");
}

#[test]
fn usd_groups_with_commas_and_cents() {
    assert_eq!(format_m(123_456, &currency::USD), "$1,234.56");
}

#[test]
fn eur_trails_the_symbol() {
    assert_eq!(format_m(1_250, &currency::EUR), "12.50 €");
}

// ── money parse ──────────────────────────────────────────────────────────

#[test]
fn idr_display_strings_roundtrip() {
    assert_eq!(parse("12.500", &currency::IDR).unwrap(), kaseer_core::money::money(12_500, "IDR"));
    assert_eq!(parse("Rp 12.500", &currency::IDR).unwrap(), kaseer_core::money::money(12_500, "IDR"));
}

#[test]
fn fractional_input_is_rejected_on_zero_decimal_currencies() {
    assert!(parse("12,5", &currency::IDR).is_err());
}

#[test]
fn usd_parses_grouping_and_cents() {
    assert_eq!(parse("1,234.56", &currency::USD).unwrap().minor, 123_456);
    assert_eq!(parse("$1,234.5", &currency::USD).unwrap().minor, 123_450);
}

#[test]
fn usd_rounds_half_up_past_the_cent() {
    assert_eq!(parse("1.234", &currency::USD).unwrap().minor, 123);
}

#[test]
fn garbage_throws() {
    assert!(parse("", &currency::IDR).is_err());
    assert!(parse("abc", &currency::IDR).is_err());
    assert!(parse("-5", &currency::IDR).is_err());
}

// ── allocate (largest remainder) ─────────────────────────────────────────

#[test]
fn hundred_over_thirds_splits_34_33_33_and_sums_exactly() {
    assert_eq!(allocate_m(100, &[1, 1, 1]), vec![34, 33, 33]);
}

#[test]
fn clean_proportional_split_is_exact() {
    assert_eq!(allocate_m(10_500, &[1, 2]), vec![3_500, 7_000]);
}

#[test]
fn degenerate_weights_split_evenly_with_remainder_on_the_first() {
    assert_eq!(allocate_m(101, &[0, 0, 0]), vec![35, 33, 33]);
}

#[test]
fn no_weights_no_parts() {
    assert_eq!(allocate_m(100, &[]), Vec::<i64>::new());
}

// ── roundHalfUpDiv (exact integer half-up) ───────────────────────────────

#[test]
fn exact_halves_round_up_without_float_drift() {
    assert_eq!(round_half_up_div(5, 2), 3);
    assert_eq!(round_half_up_div(10_505, 100), 105);
    assert_eq!(round_half_up_div(10_550, 100), 106);
}

#[test]
fn whole_and_zero() {
    assert_eq!(round_half_up_div(36_000, 1_000), 36);
    assert_eq!(round_half_up_div(0, 7), 0);
}

// ── currency registry ────────────────────────────────────────────────────

#[test]
fn idr_has_no_minor_digits_and_carries_rupiah_notes() {
    assert_eq!(currency::IDR.decimals, 0);
    assert!(currency::IDR.cash_notes.contains(&100_000));
}

#[test]
fn registry_entries_are_unique_and_known_codes_resolve() {
    let codes: Vec<_> = CURRENCIES.iter().map(|c| c.code).collect();
    let unique: std::collections::HashSet<_> = codes.iter().collect();
    assert_eq!(unique.len(), codes.len());
    assert_eq!(currency_by_code("MYR").unwrap().decimals, 2);
}

// ── tax ──────────────────────────────────────────────────────────────────

#[test]
fn exclusive_adds_on_top() {
    assert_eq!(tax(10_000, Some(&ppn11_exc())), 1_100);
}

#[test]
fn inclusive_extracts_the_portion() {
    // 11000 @ 11% -> 1090
    assert_eq!(tax(11_000, Some(&ppn11())), 1_090);
}

#[test]
fn null_rate_is_untaxed() {
    assert_eq!(tax(10_000, None), 0);
}

// ── cart totals pipeline ─────────────────────────────────────────────────

#[test]
fn gross_inclusive_tax_on_the_taxed_line_only() {
    let rates = vec![ppn11()];
    let t = compute_totals(&cart_with_p1_p2(), &rates, &currency::IDR);
    assert_eq!(t.gross_minor, 104_000);
    // 36.000 @ 11% inclusive -> 36000*1100/11100 = 3567.57 -> 3568
    assert_eq!(t.tax_minor, 3_568);
    assert_eq!(t.net_minor, 104_000);
    assert_eq!(t.total_minor, 104_000);
    assert_eq!(t.item_count, 3.0);
}

#[test]
fn order_discount_allocates_exactly_across_lines_10_percent() {
    let rates = vec![ppn11()];
    let cart = set_order_discount(&cart_with_p1_p2(), Some(Discount::Percent { bps: 1_000 }));
    let t = compute_totals(&cart, &rates, &currency::IDR);
    assert_eq!(t.discount_minor, 10_400);
    assert_eq!(t.lines[0].order_discount_minor, 3_600);
    assert_eq!(t.lines[1].order_discount_minor, 6_800);
    // net 32.400 @ 11% inclusive -> 32400*1100/11100 = 3210.81 -> 3211
    assert_eq!(t.lines[0].tax_minor, 3_211);
    assert_eq!(t.lines[1].tax_minor, 0);
    assert_eq!(t.total_minor, 93_600);
}

#[test]
fn amount_discount_splits_remainder_by_largest_remainder() {
    let rates = vec![ppn11()];
    let cart = empty_cart("IDR");
    let cart = add_to_cart(&cart, &p1(), 1.0).unwrap(); // 18.000
    let cart = add_to_cart(&cart, &p2(), 1.0).unwrap();
    // Same products merge into one line per product; use distinct lines via
    // order discount over the merged base instead.
    let cart = set_order_discount(&cart, Some(Discount::Amount { minor: 1_000 }));
    let t = compute_totals(&cart, &rates, &currency::IDR);
    assert_eq!(t.discount_minor, 1_000);
    let allocated: i64 = t.lines.iter().map(|l| l.order_discount_minor).sum();
    assert_eq!(allocated, 1_000);
}

#[test]
fn same_product_merges_quantity() {
    let cart = empty_cart("IDR");
    let cart = add_to_cart(&cart, &p1(), 1.0).unwrap();
    let cart = add_to_cart(&cart, &p1(), 2.0).unwrap();
    assert_eq!(cart.lines.len(), 1);
    assert_eq!(cart.lines[0].qty, 3.0);
}

#[test]
fn quantity_rules_fractional_ok_to_three_decimals_beyond_throws() {
    let rates = vec![ppn11()];
    let cart = cart_with_p1_p2();
    let first_id = cart.lines[0].id.clone();
    let cart = set_line_qty(&cart, &first_id, 1.5).unwrap();
    assert_eq!(compute_totals(&cart, &rates, &currency::IDR).gross_minor, 95_000); // 27.000 + 68.000
    assert!(set_line_qty(&cart, &first_id, 1.2345).is_err());
    assert!(add_to_cart(&cart, &p1(), 0.0).is_err());
}

#[test]
fn exclusive_tax_lands_on_top_of_net() {
    let rates = vec![ppn11_exc()];
    let cart = empty_cart("IDR");
    let cart = add_to_cart(
        &cart,
        &AddableProduct {
            tax_rate_id: Some("ppn11x".into()),
            ..p2()
        },
        1.0,
    )
    .unwrap();
    let t = compute_totals(&cart, &rates, &currency::IDR);
    assert_eq!(t.net_minor, 68_000);
    assert_eq!(t.tax_minor, 7_480);
    assert_eq!(t.total_minor, 75_480);
}

// ── payments ─────────────────────────────────────────────────────────────

#[test]
fn cash_short_of_total_is_incomplete() {
    let p = evaluate_payments(104_000, &[new_tender(TenderMethod::Cash, 100_000, Some(100_000))]);
    assert!(!p.complete);
    assert_eq!(p.remaining_minor, 4_000);
    assert_eq!(p.change_minor, 0);
}

#[test]
fn cash_overpay_produces_change() {
    let p = evaluate_payments(104_000, &[new_tender(TenderMethod::Cash, 104_000, Some(110_000))]);
    assert!(p.complete);
    assert_eq!(p.change_minor, 6_000);
}

#[test]
fn card_plus_cash_split_yields_change_on_the_cash_leg_only() {
    let p = evaluate_payments(
        104_000,
        &[new_tender(TenderMethod::Card, 50_000, None), new_tender(TenderMethod::Cash, 60_000, Some(60_000))],
    );
    assert!(p.complete);
    assert_eq!(p.change_minor, 6_000);
}

#[test]
fn card_cannot_overpay() {
    let p = evaluate_payments(50_000, &[new_tender(TenderMethod::Card, 60_000, None)]);
    assert!(!p.complete);
    assert!(p.invalid.is_some());
}

#[test]
fn tenders_applying_past_zero_are_rejected_for_non_cash() {
    let p = evaluate_payments(
        104_000,
        &[new_tender(TenderMethod::Cash, 104_000, Some(104_000)), new_tender(TenderMethod::Card, 1_000, None)],
    );
    assert!(p.invalid.is_some());
}

#[test]
fn loyalty_rides_in_like_a_tender_and_cannot_overpay() {
    let p = evaluate_payments(
        104_000,
        &[new_tender(TenderMethod::Loyalty, 4_000, None), new_tender(TenderMethod::Cash, 100_000, Some(100_000))],
    );
    assert!(p.complete);
    assert_eq!(p.change_minor, 0);
    let over = evaluate_payments(104_000, &[new_tender(TenderMethod::Loyalty, 200_000, None)]);
    assert!(over.invalid.is_some());
}

// ── shift drawer math ────────────────────────────────────────────────────

#[test]
fn expected_is_float_plus_cash_taken_minus_change_given_cards_do_not_touch_the_drawer() {
    let sales = vec![
        sale_cash(104_000, 6_000),
        SettledSale {
            gross_minor: 50_000,
            discount_minor: 0,
            tax_minor: 0,
            total_minor: 50_000,
            tenders: vec![TenderLine { method: TenderMethod::Card, applied_minor: 50_000, change_minor: 0 }],
        },
    ];
    assert_eq!(cash_expected_minor(&fixture_shift(), &sales), 598_000);
}

#[test]
fn variance_is_counted_minus_expected() {
    assert_eq!(count_variance_minor(598_000, 603_000), 5_000);
    assert_eq!(count_variance_minor(598_000, 590_000), -8_000);
}

#[test]
fn totals_split_by_method() {
    let sales = vec![
        sale_cash(104_000, 6_000),
        SettledSale {
            gross_minor: 50_000,
            discount_minor: 0,
            tax_minor: 0,
            total_minor: 50_000,
            tenders: vec![TenderLine { method: TenderMethod::Card, applied_minor: 50_000, change_minor: 0 }],
        },
    ];
    let t = shift_totals(&sales);
    assert_eq!(t.sales_count, 2);
    assert_eq!(t.by_method.cash, 104_000);
    assert_eq!(t.by_method.card, 50_000);
    assert_eq!(t.total_minor, 154_000);
}

// ── stock ledger ─────────────────────────────────────────────────────────

#[test]
fn on_hand_is_the_sum_of_moves() {
    let moves = vec![
        StockMove { id: "m1".into(), product_id: "p".into(), delta: 25, reason: StockReason::Receive, ref_id: None, at: String::new(), note: None },
        StockMove { id: "m2".into(), product_id: "p".into(), delta: -1, reason: StockReason::Sale, ref_id: None, at: String::new(), note: None },
        StockMove { id: "m3".into(), product_id: "p".into(), delta: -2, reason: StockReason::Shrinkage, ref_id: None, at: String::new(), note: None },
    ];
    assert_eq!(stock_on_hand(&moves), 22);
}

#[test]
fn low_stock_boundary_sits_at_the_threshold_untracked_is_never_low() {
    let tracked = kaseer_core::inventory::StockTracking { track_stock: true, low_stock_at: 5 };
    assert!(is_low_stock(&tracked, 5));
    assert!(!is_low_stock(&tracked, 6));
    let untracked = kaseer_core::inventory::StockTracking { track_stock: false, low_stock_at: 0 };
    assert!(!is_low_stock(&untracked, 0));
}

// ── loyalty ──────────────────────────────────────────────────────────────

fn loyalty_cfg() -> LoyaltyConfig {
    LoyaltyConfig { enabled: true, step_minor: 10_000, points_per_step: 1, point_value_minor: 100 }
}

#[test]
fn earns_whole_steps_only() {
    let cfg = loyalty_cfg();
    assert_eq!(points_earned(25_000, &cfg), 2);
    assert_eq!(points_earned(9_999, &cfg), 0);
}

#[test]
fn disabled_program_earns_nothing() {
    let cfg = LoyaltyConfig { enabled: false, ..loyalty_cfg() };
    assert_eq!(points_earned(25_000, &cfg), 0);
}

#[test]
fn redemption_caps_at_points_and_at_the_order_net() {
    let cfg = loyalty_cfg();
    assert_eq!(max_redeemable_points(100, 5_000, &cfg), 50);
    assert_eq!(max_redeemable_points(10, 5_000, &cfg), 10);
    assert_eq!(max_redeemable_points(100, 0, &cfg), 0);
}

// ── receipt ──────────────────────────────────────────────────────────────

#[test]
fn daily_sequence_numbering() {
    assert_eq!(sale_number("2026-09-26T17:27:00.000Z", 1, "KSR"), "KSR-20260926-0001");
    assert_eq!(sale_number("2026-09-26T17:27:00.000Z", 47, "KSR"), "KSR-20260926-0047");
}

#[test]
fn renders_inside_the_paper_width_with_totals_and_change() {
    let store = StoreInfo {
        name: "Kaseer Demo Store".into(),
        address: Some("Jl. Merdeka 123, Malang".into()),
        footer: Some("Thank you!".into()),
    };
    let text = render_receipt_text(&fixture_sale(), &store, &currency::IDR, 42);
    for line in text.split('\n') {
        assert!(line.chars().count() <= 42, "line too wide: {:?}", line);
    }
    assert!(text.contains("KASEER DEMO STORE"));
    assert!(text.contains("TOTAL"));
    assert!(text.contains("Rp 104.000"));
    assert!(text.contains("Change"));
    assert!(text.contains("+10"));
}

#[test]
fn long_item_names_wrap_instead_of_overflowing() {
    let mut sale = fixture_sale();
    sale.lines[0].name = "Extra Long Product Name That Keeps Going On And On Forever".into();
    let text = render_receipt_text(&sale, &StoreInfo::named("Kaseer Demo Store"), &currency::IDR, 42);
    for line in text.split('\n') {
        assert!(line.chars().count() <= 42, "line too wide: {:?}", line);
    }
}
