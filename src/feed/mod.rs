//! CEX price feeds (currently Binance only).

pub mod binance;

/// Snapshot of a CEX top-of-book at a specific instant.
/// All prices in USD per base unit (e.g., USD per SOL).
#[derive(Debug, Clone, Copy)]
pub struct PriceSnapshot {
    pub best_bid_usd: f64,
    pub best_ask_usd: f64,
    /// Local receive time, NOT exchange timestamp (avoids clock skew).
    pub received_at: std::time::Instant,
}

impl PriceSnapshot {
    pub fn mid(&self) -> f64 {
        (self.best_bid_usd + self.best_ask_usd) / 2.0
    }

    pub fn age_ms(&self) -> u64 {
        self.received_at.elapsed().as_millis() as u64
    }
}
