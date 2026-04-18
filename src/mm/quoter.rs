//! Pure-function quoting logic for market making.
//!
//! Given a CEX reference mid, inventory state, and the configured spread +
//! skew parameters, produce a (bid, ask) price pair to post on-chain. All
//! prices are in HUMAN units (e.g., USDC per SOL).
//!
//! The quoter is deliberately stateless — the orchestration layer decides
//! whether to actually re-post based on the current book state.

/// Configuration for the quoter.
#[derive(Debug, Clone)]
pub struct QuoterConfig {
    /// Half-spread as a fraction of mid. 0.0005 = 5 bps each side (10 bps total).
    pub half_spread_frac: f64,
    /// Maximum absolute skew applied when inventory is away from the target.
    /// 0.001 = ±10 bps shift in bid/ask midpoint based on inventory.
    pub max_skew_frac: f64,
    /// Inventory ratio (base/total in USD terms) considered "neutral".
    /// Typically 0.5 for a balanced book.
    pub target_inventory_ratio: f64,
    /// Full-skew threshold: how far from target before max skew kicks in.
    /// 0.3 means at ratio=0.2 or 0.8 (i.e., 0.3 away from 0.5), skew is maxed.
    pub skew_ratio_window: f64,
    /// Minimum half-spread after skew — never quote tighter than this.
    pub min_half_spread_frac: f64,
    /// Order size in BASE atoms (both bid and ask use same size).
    pub order_size_base_atoms: u64,
}

/// A single (bid, ask) decision from the quoter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuoteDecision {
    /// Bid price (human units, e.g., USDC per SOL). We BUY at this price.
    pub bid_price: f64,
    /// Ask price. We SELL at this price.
    pub ask_price: f64,
    /// Bid size in BASE atoms.
    pub bid_size_base_atoms: u64,
    /// Ask size in BASE atoms.
    pub ask_size_base_atoms: u64,
}

/// Stateless quoter — converts a CEX mid + inventory ratio into (bid, ask).
#[derive(Debug)]
pub struct Quoter {
    pub cfg: QuoterConfig,
}

impl Quoter {
    pub fn new(cfg: QuoterConfig) -> Self {
        Self { cfg }
    }

    /// Compute (bid, ask) given:
    ///   - `cex_mid`: CEX reference price in quote units per base unit
    ///   - `inventory_ratio`: current base-side share of total portfolio in USD
    ///     (0.0 = all quote, 1.0 = all base, 0.5 = balanced)
    ///
    /// Skew logic:
    ///   - If we're BASE-heavy (ratio > target), shift mid DOWN so our ask is
    ///     more aggressive (we want to SELL base) and our bid is less aggressive
    ///     (we don't want more base).
    ///   - If we're QUOTE-heavy (ratio < target), shift mid UP so our bid is
    ///     more aggressive (we want to BUY base) and our ask is less aggressive.
    pub fn quote(&self, cex_mid: f64, inventory_ratio: f64) -> QuoteDecision {
        let skew_bps = self.skew_fraction(inventory_ratio);
        let adjusted_mid = cex_mid * (1.0 + skew_bps);

        let half_spread = self.cfg.half_spread_frac.max(self.cfg.min_half_spread_frac);
        let bid = adjusted_mid * (1.0 - half_spread);
        let ask = adjusted_mid * (1.0 + half_spread);

        QuoteDecision {
            bid_price: bid,
            ask_price: ask,
            bid_size_base_atoms: self.cfg.order_size_base_atoms,
            ask_size_base_atoms: self.cfg.order_size_base_atoms,
        }
    }

    /// Signed skew as a fraction of mid.
    ///
    /// At `inventory_ratio = target`, returns 0.
    /// As ratio moves toward 0 (all-quote), returns +max_skew (shift mid up → we buy more aggressively).
    /// As ratio moves toward 1 (all-base), returns -max_skew (shift mid down → we sell more aggressively).
    fn skew_fraction(&self, inventory_ratio: f64) -> f64 {
        let delta = inventory_ratio - self.cfg.target_inventory_ratio;
        if self.cfg.skew_ratio_window <= 0.0 {
            return 0.0;
        }
        // Normalize to [-1, +1] based on window.
        let normalized = (-delta / self.cfg.skew_ratio_window).clamp(-1.0, 1.0);
        normalized * self.cfg.max_skew_frac
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_cfg() -> QuoterConfig {
        QuoterConfig {
            half_spread_frac: 0.0005, // 5 bps each side = 10 bps total
            max_skew_frac: 0.001,     // 10 bps max skew
            target_inventory_ratio: 0.5,
            skew_ratio_window: 0.3, // full skew at ratio=0.2 or 0.8
            min_half_spread_frac: 0.0002,
            order_size_base_atoms: 100_000_000, // 0.1 SOL if base=SOL
        }
    }

    #[test]
    fn neutral_inventory_produces_symmetric_quotes() {
        let q = Quoter::new(default_cfg());
        let d = q.quote(100.0, 0.5);
        assert!((d.bid_price - 99.95).abs() < 1e-6);
        assert!((d.ask_price - 100.05).abs() < 1e-6);
        assert_eq!(d.bid_size_base_atoms, 100_000_000);
        assert_eq!(d.ask_size_base_atoms, 100_000_000);
    }

    #[test]
    fn base_heavy_inventory_shifts_mid_down() {
        let q = Quoter::new(default_cfg());
        let d = q.quote(100.0, 0.8); // full base-heavy, at window edge
        // skew = -max = -10 bps → adjusted mid = 99.90
        // bid = 99.90 * 0.9995 = 99.85005
        // ask = 99.90 * 1.0005 = 99.94995
        assert!((d.bid_price - 99.85005).abs() < 1e-3);
        assert!((d.ask_price - 99.94995).abs() < 1e-3);
        assert!(d.ask_price < 100.0, "ask must be below mid when base-heavy");
    }

    #[test]
    fn quote_heavy_inventory_shifts_mid_up() {
        let q = Quoter::new(default_cfg());
        let d = q.quote(100.0, 0.2); // quote-heavy
        // skew = +max = +10 bps → adjusted mid = 100.10
        // bid = 100.10 * 0.9995 ≈ 100.05
        assert!(d.bid_price > 100.0, "bid must be above mid when quote-heavy");
        assert!(d.ask_price > 100.1, "ask should be above mid + regular half-spread when quote-heavy");
    }

    #[test]
    fn skew_clamps_beyond_window() {
        let q = Quoter::new(default_cfg());
        // Past window (ratio=0.0 means quote_heavy to the max) — skew should be capped.
        let d_extreme = q.quote(100.0, 0.0);
        let d_boundary = q.quote(100.0, 0.2);
        assert!(
            (d_extreme.bid_price - d_boundary.bid_price).abs() < 1e-6,
            "skew should clamp beyond window"
        );
    }

    #[test]
    fn min_half_spread_is_honored() {
        let mut cfg = default_cfg();
        cfg.half_spread_frac = 0.00001; // 0.1 bps — below min
        cfg.min_half_spread_frac = 0.0002; // 2 bps floor
        let q = Quoter::new(cfg);
        let d = q.quote(100.0, 0.5);
        // bid = 100 * (1 - 0.0002) = 99.98, ask = 100.02
        assert!((d.bid_price - 99.98).abs() < 1e-4);
        assert!((d.ask_price - 100.02).abs() < 1e-4);
    }

    #[test]
    fn ask_always_greater_than_bid() {
        let q = Quoter::new(default_cfg());
        for ratio in [0.0, 0.2, 0.4, 0.5, 0.6, 0.8, 1.0] {
            let d = q.quote(100.0, ratio);
            assert!(
                d.ask_price > d.bid_price,
                "ratio={} bid={} ask={}",
                ratio,
                d.bid_price,
                d.ask_price
            );
        }
    }
}
