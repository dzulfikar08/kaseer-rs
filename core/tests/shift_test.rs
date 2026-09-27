//! Ported from kaseer/src/core/shift.test.ts (countToMinor suite).

use kaseer_core::shift::{count_to_minor, DenominationCount};

#[test]
fn empty_count_is_zero() {
    assert_eq!(count_to_minor(&[]), 0);
}

#[test]
fn single_denomination() {
    assert_eq!(count_to_minor(&[DenominationCount { minor: 10_000, count: 3 }]), 30_000);
}

#[test]
fn full_idr_drawer_count() {
    let counts = vec![
        DenominationCount { minor: 2_000, count: 4 },
        DenominationCount { minor: 5_000, count: 0 },
        DenominationCount { minor: 10_000, count: 12 },
        DenominationCount { minor: 20_000, count: 1 },
        DenominationCount { minor: 50_000, count: 2 },
        DenominationCount { minor: 100_000, count: 5 },
    ];
    assert_eq!(count_to_minor(&counts), 748_000);
}

#[test]
fn sub_major_denominations_sum_exactly() {
    assert_eq!(
        count_to_minor(&[DenominationCount { minor: 25, count: 4 }, DenominationCount { minor: 10, count: 1 }]),
        110
    );
}
