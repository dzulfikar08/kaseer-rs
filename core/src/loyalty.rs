//! Loyalty points. Earned on net (post-discount, pre-tax) spend in whole
//! steps; redeemed at a flat minor value per point. Configured, not hard
//! coded, so a store can turn the program off entirely.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoyaltyConfig {
    pub enabled: bool,
    /// Spend this many minor units to earn one step.
    pub step_minor: i64,
    pub points_per_step: i64,
    /// Minor units one point is worth at redemption.
    pub point_value_minor: i64,
}

pub fn points_earned(net_minor: i64, cfg: &LoyaltyConfig) -> i64 {
    if !cfg.enabled || cfg.step_minor <= 0 || net_minor <= 0 {
        return 0;
    }
    net_minor / cfg.step_minor * cfg.points_per_step
}

pub fn redeem_value_minor(points: i64, cfg: &LoyaltyConfig) -> i64 {
    if points <= 0 {
        return 0;
    }
    points * cfg.point_value_minor
}

/// Redeem as many whole points as possible without exceeding `net_minor`.
pub fn max_redeemable_points(points: i64, net_minor: i64, cfg: &LoyaltyConfig) -> i64 {
    if !cfg.enabled || points <= 0 || cfg.point_value_minor <= 0 {
        return 0;
    }
    points.min(net_minor / cfg.point_value_minor)
}
