//! Stock as an append-only movement ledger. stock_on_hand is derived,
//! never stored and mutated, so every discrepancy is auditable after the
//! fact.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockReason {
    Sale,
    Receive,
    Adjust,
    Refund,
    Shrinkage,
}

impl StockReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StockReason::Sale => "sale",
            StockReason::Receive => "receive",
            StockReason::Adjust => "adjust",
            StockReason::Refund => "refund",
            StockReason::Shrinkage => "shrinkage",
        }
    }

    pub fn parse(s: &str) -> Option<StockReason> {
        match s {
            "sale" => Some(StockReason::Sale),
            "receive" => Some(StockReason::Receive),
            "adjust" => Some(StockReason::Adjust),
            "refund" => Some(StockReason::Refund),
            "shrinkage" => Some(StockReason::Shrinkage),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockMove {
    pub id: String,
    pub product_id: String,
    pub delta: i64,
    pub reason: StockReason,
    /// Sale id for sale/refund moves; free text reference otherwise.
    pub ref_id: Option<String>,
    pub at: String,
    pub note: Option<String>,
}

pub fn stock_on_hand(moves: &[StockMove]) -> i64 {
    moves.iter().map(|m| m.delta).sum()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockTracking {
    pub track_stock: bool,
    pub low_stock_at: i64,
}

pub fn is_low_stock(product: &StockTracking, on_hand: i64) -> bool {
    product.track_stock && on_hand <= product.low_stock_at
}
