//! Shared price state for CEX and on-chain pools.
//!
//! Written to by the Binance WS client (CEX prices) and the narrow
//! Geyser subscriber (pool states, via the existing StateCache).
//! Read by the detector.

use dashmap::DashMap;
use std::sync::Arc;

use crate::feed::PriceSnapshot;
use crate::state::StateCache;

/// Thread-safe store for CEX top-of-book snapshots and DEX pool state.
///
/// Cloning is cheap — all clones share the same underlying maps via `Arc`.
#[derive(Clone)]
pub struct PriceStore {
    /// CEX prices keyed by symbol (e.g., "SOLUSDC").
    cex: Arc<DashMap<String, PriceSnapshot>>,
    /// Pool state cache (reuses main engine type).
    pub pools: StateCache,
}

impl Default for PriceStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PriceStore {
    pub fn new() -> Self {
        Self {
            cex: Arc::new(DashMap::new()),
            pools: StateCache::new(std::time::Duration::from_secs(5)),
        }
    }

    /// Construct with a pre-existing `StateCache` (e.g., shared with the main engine).
    pub fn with_state_cache(pools: StateCache) -> Self {
        Self {
            cex: Arc::new(DashMap::new()),
            pools,
        }
    }

    /// Insert or replace the CEX snapshot for `symbol` (e.g. `"SOLUSDC"`).
    pub fn update_cex(&self, symbol: &str, snapshot: PriceSnapshot) {
        self.cex.insert(symbol.to_string(), snapshot);
    }

    /// Return the most recent CEX snapshot for `symbol`, or `None` if never received.
    pub fn get_cex(&self, symbol: &str) -> Option<PriceSnapshot> {
        self.cex.get(symbol).map(|v| *v.value())
    }

    /// Returns `true` if the CEX snapshot for `symbol` is older than `max_age_ms`,
    /// or if no snapshot has ever been received for that symbol.
    pub fn is_stale(&self, symbol: &str, max_age_ms: u64) -> bool {
        match self.cex.get(symbol) {
            Some(snap) => snap.age_ms() > max_age_ms,
            None => true,
        }
    }
}
