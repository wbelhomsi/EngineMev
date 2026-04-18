//! Local in-memory tracking of our own live orders on the Manifest market.
//!
//! Manifest assigns a `order_sequence_number: u64` to every placed order. We
//! track these locally so we know what to cancel on the next requote cycle,
//! and we can detect fills by noticing orders that have disappeared from
//! the on-chain book.
//!
//! This is NOT a full orderbook replica — we only track orders we placed.
//! Use the parser in `src/mempool/parsers/manifest.rs` for top-of-book.

use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Bid,
    Ask,
}

/// Metadata about an order we've placed that may still be resting on-chain.
#[derive(Debug, Clone)]
pub struct LiveOrder {
    pub seq_number: u64,
    pub side: OrderSide,
    pub price_mantissa: u32,
    pub price_exponent: i8,
    pub base_atoms: u64,
    /// When we submitted the place IX. Used to age out stale entries if an
    /// order goes missing without a clear fill or cancel event.
    pub placed_at: Instant,
}

impl LiveOrder {
    /// Reconstruct the human price from mantissa + exponent.
    /// Price is quote_atoms per base_atom on the wire; this returns raw.
    pub fn price_raw(&self) -> f64 {
        (self.price_mantissa as f64) * 10f64.powi(self.price_exponent as i32)
    }
}

/// The set of orders we believe we have resting on the market.
///
/// Not thread-safe; wrap in Mutex if shared across tasks.
#[derive(Debug, Default)]
pub struct BookState {
    pub orders: Vec<LiveOrder>,
}

impl BookState {
    pub fn new() -> Self {
        Self { orders: Vec::new() }
    }

    /// Record a newly-placed order.
    pub fn insert(&mut self, order: LiveOrder) {
        self.orders.push(order);
    }

    /// Remove orders by sequence number. Returns the removed orders.
    pub fn remove_many(&mut self, seq_numbers: &[u64]) -> Vec<LiveOrder> {
        let mut removed = Vec::new();
        self.orders.retain(|o| {
            if seq_numbers.contains(&o.seq_number) {
                removed.push(o.clone());
                false
            } else {
                true
            }
        });
        removed
    }

    /// All currently-tracked sequence numbers.
    pub fn seq_numbers(&self) -> Vec<u64> {
        self.orders.iter().map(|o| o.seq_number).collect()
    }

    /// All bids we believe are live.
    pub fn bids(&self) -> impl Iterator<Item = &LiveOrder> {
        self.orders.iter().filter(|o| o.side == OrderSide::Bid)
    }

    /// All asks we believe are live.
    pub fn asks(&self) -> impl Iterator<Item = &LiveOrder> {
        self.orders.iter().filter(|o| o.side == OrderSide::Ask)
    }

    pub fn len(&self) -> usize {
        self.orders.len()
    }

    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }

    pub fn clear(&mut self) {
        self.orders.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(seq: u64, side: OrderSide) -> LiveOrder {
        LiveOrder {
            seq_number: seq,
            side,
            price_mantissa: 42,
            price_exponent: -2,
            base_atoms: 1_000_000,
            placed_at: Instant::now(),
        }
    }

    #[test]
    fn insert_and_iterate() {
        let mut bs = BookState::new();
        bs.insert(make(1, OrderSide::Bid));
        bs.insert(make(2, OrderSide::Ask));
        bs.insert(make(3, OrderSide::Bid));
        assert_eq!(bs.len(), 3);
        assert_eq!(bs.bids().count(), 2);
        assert_eq!(bs.asks().count(), 1);
    }

    #[test]
    fn remove_many_returns_matches() {
        let mut bs = BookState::new();
        bs.insert(make(1, OrderSide::Bid));
        bs.insert(make(2, OrderSide::Ask));
        bs.insert(make(3, OrderSide::Bid));
        let removed = bs.remove_many(&[1, 3]);
        assert_eq!(removed.len(), 2);
        assert_eq!(bs.len(), 1);
        assert_eq!(bs.orders[0].seq_number, 2);
    }

    #[test]
    fn seq_numbers_returns_all() {
        let mut bs = BookState::new();
        bs.insert(make(10, OrderSide::Bid));
        bs.insert(make(20, OrderSide::Ask));
        let mut seqs = bs.seq_numbers();
        seqs.sort();
        assert_eq!(seqs, vec![10, 20]);
    }

    #[test]
    fn price_raw_reconstructs_value() {
        let o = make(1, OrderSide::Bid);
        // mantissa=42, exponent=-2 → 0.42
        assert!((o.price_raw() - 0.42).abs() < 1e-9);
    }
}
