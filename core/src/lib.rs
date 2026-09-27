//! Kaseer POS domain core: pure money-safe retail arithmetic plus the
//! SQLite persistence layer. Integer minor units everywhere; every
//! rounding decision is exact integer half-up.

/// Re-export so crates without their own chrono dep can still name the
/// `DateTime<Utc>` that copilot/insights take as an argument.
pub use chrono;

pub mod analytics;
pub mod bundles;
pub mod campaigns;
pub mod cart;
pub mod copilot;
pub mod currency;
pub mod db;
pub mod escpos;
pub mod insights;
pub mod inventory;
pub mod loyalty;
pub mod money;
pub mod opportunities;
pub mod payments;
pub mod product_perf;
pub mod receipt;
pub mod repos;
pub mod reports;
pub mod segments;
pub mod seed_data;
pub mod shift;
pub mod tax;
pub mod winback;
