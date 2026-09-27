//! Ported from kaseer/src/core/reports.test.ts — range-series tests for
//! byDay/byWeek/byMonth. Sale fixtures carry fixed `at` stamps so bucket
//! labels and sums assert exactly, no clock reliance.

mod common;

use common::bare_sale;
use kaseer_core::reports::{by_day, by_month, by_week, PeriodBucket};

fn sale(at: &str, total_minor: i64) -> kaseer_core::receipt::Sale {
    bare_sale(at, None, vec![], total_minor, 0)
}

fn bucket(day: &str, total_minor: i64, count: i64) -> PeriodBucket {
    PeriodBucket { day: day.to_string(), total_minor, count }
}

#[test]
fn by_day_buckets_per_date_sums_exactly_zero_fills_gap_days_drops_out_of_range_sales() {
    let sales = [
        sale("2026-09-23T23:59:00.000Z", 999), // before the range: excluded
        sale("2026-09-24T10:00:00.000Z", 5000),
        sale("2026-09-24T18:30:00.000Z", 2500),
        sale("2026-09-26T09:00:00.000Z", 1250),
        sale("2026-09-27T00:00:00.000Z", 1), // after the range: excluded
    ];
    let got = by_day(&sales, "2026-09-24T00:00:00.000Z", "2026-09-26T12:00:00.000Z");
    assert_eq!(got, vec![bucket("2026-09-24", 7500, 2), bucket("2026-09-25", 0, 0), bucket("2026-09-26", 1250, 1)]);
}

#[test]
fn by_day_same_day_boundaries_land_in_buckets() {
    let sales = [sale("2026-09-24T00:00:00.000Z", 100), sale("2026-09-26T23:59:59.999Z", 200)];
    let got = by_day(&sales, "2026-09-24T00:00:00.000Z", "2026-09-26T23:59:59.999Z");
    assert_eq!(got, vec![bucket("2026-09-24", 100, 1), bucket("2026-09-25", 0, 0), bucket("2026-09-26", 200, 1)]);
}

#[test]
fn by_week_labels_iso_weeks_and_keeps_monday_rollover_in_order() {
    let sales = [
        sale("2026-09-21T06:00:00.000Z", 111), // Monday before the range: excluded
        sale("2026-09-22T10:00:00.000Z", 3000), // Tuesday of W39
        sale("2026-09-27T20:00:00.000Z", 700), // Sunday, still W39
        sale("2026-09-28T08:00:00.000Z", 1500), // Monday, starts W40
    ];
    let got = by_week(&sales, "2026-09-22T00:00:00.000Z", "2026-09-28T23:00:00.000Z");
    assert_eq!(got, vec![bucket("2026-W39", 3700, 2), bucket("2026-W40", 1500, 1)]);
}

#[test]
fn by_week_zero_fills_every_iso_week_spanned_by_a_30_day_like_range() {
    let sales = [sale("2026-09-10T10:00:00.000Z", 100)]; // Thursday of W37
    let got = by_week(&sales, "2026-08-29T00:00:00.000Z", "2026-09-27T12:00:00.000Z");
    assert_eq!(
        got,
        vec![
            bucket("2026-W35", 0, 0),
            bucket("2026-W36", 0, 0),
            bucket("2026-W37", 100, 1),
            bucket("2026-W38", 0, 0),
            bucket("2026-W39", 0, 0),
        ]
    );
}

#[test]
fn by_week_iso_year_rolls_on_the_week_holding_jan_4() {
    let sales = [sale("2025-12-29T10:00:00.000Z", 500), sale("2026-01-01T10:00:00.000Z", 700)];
    let got = by_week(&sales, "2025-12-29T00:00:00.000Z", "2026-01-01T12:00:00.000Z");
    assert_eq!(got, vec![bucket("2026-W01", 1200, 2)]);
}

#[test]
fn by_month_buckets_across_the_month_seam() {
    let sales = [
        sale("2026-08-31T22:00:00.000Z", 4000),
        sale("2026-09-01T00:30:00.000Z", 250),
        sale("2026-09-15T12:00:00.000Z", 250),
    ];
    let got = by_month(&sales, "2026-08-31T00:00:00.000Z", "2026-09-15T12:00:00.000Z");
    assert_eq!(got, vec![bucket("2026-08", 4000, 1), bucket("2026-09", 500, 2)]);
}

#[test]
fn by_month_enumerates_every_month_in_a_multi_month_range_ascending() {
    let sales = [sale("2025-11-02T10:00:00.000Z", 60), sale("2026-01-20T10:00:00.000Z", 40)];
    let got = by_month(&sales, "2025-11-01T00:00:00.000Z", "2026-01-31T12:00:00.000Z");
    assert_eq!(
        got,
        vec![bucket("2025-11", 60, 1), bucket("2025-12", 0, 0), bucket("2026-01", 40, 1)]
    );
}
