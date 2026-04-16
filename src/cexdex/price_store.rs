//! PriceStore — shared CEX and DEX price state.
//!
//! Minimal stub for Task 3 (Binance WS client). Full implementation in Task 4.

use std::sync::Arc;

use dashmap::DashMap;

use crate::feed::PriceSnapshot;

/// Thread-safe store for CEX top-of-book snapshots (and future DEX state).
/// Cloning is cheap — all clones share the same underlying maps.
#[derive(Clone, Default)]
pub struct PriceStore {
    cex: Arc<DashMap<String, PriceSnapshot>>,
}

impl PriceStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace the CEX snapshot for `symbol` (e.g. `"SOLUSDC"`).
    pub(crate) fn update_cex(&self, symbol: &str, snapshot: PriceSnapshot) {
        self.cex.insert(symbol.to_string(), snapshot);
    }
}
