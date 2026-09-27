//! First-run seed data: a demo catalog, default tax rate, and default
//! settings. Prices live in the store currency's minor units (IDR default:
//! 1 minor = 1 rupiah). Skus are EAN-13-shaped so scanner entry works out
//! of the box.

use crate::loyalty::LoyaltyConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedTaxRate {
    pub id: &'static str,
    pub name: &'static str,
    pub bps: i64,
    pub inclusive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedProduct {
    pub sku: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub price_minor: i64,
    pub cost_minor: i64,
    pub tax_rate_id: Option<&'static str>,
    pub track_stock: bool,
    pub stock: i64,
    pub low_stock_at: i64,
}

pub const DEFAULT_TAX_RATES: &[SeedTaxRate] = &[SeedTaxRate {
    id: "ppn11",
    name: "PPN 11%",
    bps: 1_100,
    inclusive: true,
}];

pub const DEFAULT_STORE_NAME: &str = "Kaseer Demo Store";
pub const DEFAULT_ADDRESS: &str = "Jl. Merdeka 123, Malang";
pub const DEFAULT_RECEIPT_FOOTER: &str = "Thank you for shopping with us!";
pub const DEFAULT_CURRENCY_CODE: &str = "IDR";
pub const DEFAULT_LOYALTY: LoyaltyConfig = LoyaltyConfig {
    enabled: true,
    step_minor: 10_000,
    points_per_step: 1,
    point_value_minor: 100,
};

pub const SEED_CATEGORIES: &[&str] = &["Coffee", "Food", "Snacks", "Drinks", "Groceries"];

pub const SEED_PRODUCTS: &[SeedProduct] = &[
    SeedProduct { sku: "8991002100011", name: "Kopi Susu Gula Aren", category: "Coffee", price_minor: 18_000, cost_minor: 6_500, tax_rate_id: Some("ppn11"), track_stock: false, stock: 0, low_stock_at: 0 },
    SeedProduct { sku: "8991002100028", name: "Americano", category: "Coffee", price_minor: 15_000, cost_minor: 5_000, tax_rate_id: Some("ppn11"), track_stock: false, stock: 0, low_stock_at: 0 },
    SeedProduct { sku: "8991002100035", name: "Kopi Hitam Tubruk", category: "Coffee", price_minor: 12_000, cost_minor: 4_000, tax_rate_id: Some("ppn11"), track_stock: false, stock: 0, low_stock_at: 0 },
    SeedProduct { sku: "8991002100042", name: "Es Kopi Coklat", category: "Coffee", price_minor: 22_000, cost_minor: 8_000, tax_rate_id: Some("ppn11"), track_stock: false, stock: 0, low_stock_at: 0 },
    SeedProduct { sku: "8991002100059", name: "Matcha Latte", category: "Coffee", price_minor: 25_000, cost_minor: 10_000, tax_rate_id: Some("ppn11"), track_stock: false, stock: 0, low_stock_at: 0 },

    SeedProduct { sku: "8991002101018", name: "Nasi Goreng Spesial", category: "Food", price_minor: 25_000, cost_minor: 12_000, tax_rate_id: Some("ppn11"), track_stock: false, stock: 0, low_stock_at: 0 },
    SeedProduct { sku: "8991002101025", name: "Mie Goreng", category: "Food", price_minor: 22_000, cost_minor: 10_000, tax_rate_id: Some("ppn11"), track_stock: false, stock: 0, low_stock_at: 0 },
    SeedProduct { sku: "8991002101032", name: "Ayam Geprek + Nasi", category: "Food", price_minor: 27_000, cost_minor: 14_000, tax_rate_id: Some("ppn11"), track_stock: false, stock: 0, low_stock_at: 0 },
    SeedProduct { sku: "8991002101049", name: "Roti Bakar Coklat", category: "Food", price_minor: 15_000, cost_minor: 6_000, tax_rate_id: Some("ppn11"), track_stock: false, stock: 0, low_stock_at: 0 },
    SeedProduct { sku: "8991002101056", name: "Pisang Goreng (3 pcs)", category: "Food", price_minor: 12_000, cost_minor: 5_000, tax_rate_id: Some("ppn11"), track_stock: true, stock: 40, low_stock_at: 10 },

    SeedProduct { sku: "8991002102017", name: "Kentang Goreng", category: "Snacks", price_minor: 15_000, cost_minor: 6_000, tax_rate_id: Some("ppn11"), track_stock: false, stock: 0, low_stock_at: 0 },
    SeedProduct { sku: "8991002102024", name: "Tempe Mendoan", category: "Snacks", price_minor: 10_000, cost_minor: 4_000, tax_rate_id: Some("ppn11"), track_stock: true, stock: 30, low_stock_at: 8 },
    SeedProduct { sku: "8991002102031", name: "Tahu Crispy", category: "Snacks", price_minor: 10_000, cost_minor: 4_000, tax_rate_id: Some("ppn11"), track_stock: true, stock: 30, low_stock_at: 8 },

    SeedProduct { sku: "8991002103016", name: "Es Teh Manis", category: "Drinks", price_minor: 6_000, cost_minor: 1_500, tax_rate_id: Some("ppn11"), track_stock: false, stock: 0, low_stock_at: 0 },
    SeedProduct { sku: "8991002103023", name: "Es Jeruk", category: "Drinks", price_minor: 8_000, cost_minor: 2_500, tax_rate_id: Some("ppn11"), track_stock: false, stock: 0, low_stock_at: 0 },
    SeedProduct { sku: "8991002103030", name: "Air Mineral 600ml", category: "Drinks", price_minor: 4_000, cost_minor: 2_000, tax_rate_id: Some("ppn11"), track_stock: true, stock: 120, low_stock_at: 24 },
    SeedProduct { sku: "8991002103047", name: "Susu Kotak Coklat", category: "Drinks", price_minor: 6_500, cost_minor: 4_200, tax_rate_id: Some("ppn11"), track_stock: true, stock: 48, low_stock_at: 12 },

    SeedProduct { sku: "8991002104015", name: "Beras Premium 5kg", category: "Groceries", price_minor: 68_000, cost_minor: 58_000, tax_rate_id: None, track_stock: true, stock: 25, low_stock_at: 5 },
    SeedProduct { sku: "8991002104022", name: "Gula Pasir 1kg", category: "Groceries", price_minor: 17_000, cost_minor: 14_500, tax_rate_id: None, track_stock: true, stock: 30, low_stock_at: 6 },
    SeedProduct { sku: "8991002104039", name: "Minyak Goreng 1L", category: "Groceries", price_minor: 19_000, cost_minor: 16_500, tax_rate_id: None, track_stock: true, stock: 28, low_stock_at: 6 },
    SeedProduct { sku: "8991002104046", name: "Telur Ayam 1kg", category: "Groceries", price_minor: 28_000, cost_minor: 24_000, tax_rate_id: None, track_stock: true, stock: 20, low_stock_at: 5 },
    SeedProduct { sku: "8991002104053", name: "Mie Instan Goreng", category: "Groceries", price_minor: 3_500, cost_minor: 2_800, tax_rate_id: None, track_stock: true, stock: 144, low_stock_at: 24 },
    SeedProduct { sku: "8991002104060", name: "Kopi Sachet 10x", category: "Groceries", price_minor: 14_000, cost_minor: 11_500, tax_rate_id: None, track_stock: true, stock: 36, low_stock_at: 8 },
    SeedProduct { sku: "8991002104077", name: "Tepung Terigu 1kg", category: "Groceries", price_minor: 13_000, cost_minor: 11_000, tax_rate_id: None, track_stock: true, stock: 22, low_stock_at: 5 },
];
