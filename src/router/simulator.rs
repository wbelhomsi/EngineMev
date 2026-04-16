use tracing::debug;

use crate::router::pool::{ArbRoute, DexType};
use crate::state::{StateCache, TipFloorCache};

/// Final profit simulation before bundle submission.
///
/// The RouteCalculator does fast approximate math to find candidates.
/// The ProfitSimulator does precise validation:
/// - Re-reads freshest pool state from cache
/// - Accounts for exact fees and tick-crossing (CLMM)
/// - Calculates tip amount based on profit
/// - Uses dynamic Jito tip floor (polled from REST API) as minimum
/// - Returns a go/no-go decision
///
/// This is the last gate before we spend money (Jito tip).
/// If simulation says no-go, we drop the opportunity. No partial bets.
pub struct ProfitSimulator {
    state_cache: StateCache,
    tip_fraction: f64,
    min_profit_lamports: u64,
    /// Static minimum tip from config (fallback when tip floor API is unavailable).
    min_tip_lamports: u64,
    /// Slippage tolerance (0.0 - 1.0). Plan as if we realize (1 - slippage) of gross profit.
    slippage_tolerance: f64,
    /// Dynamic tip floor from Jito REST API (overrides min_tip_lamports when available).
    tip_floor_cache: Option<TipFloorCache>,
}

/// Default slippage tolerance applied to gross profit (overridden by SLIPPAGE_TOLERANCE env).
/// With 25% slippage, we plan as if we'll only realize 75% of estimated profit.
/// This makes tip calculation conservative and sets a tighter min_final_output.
const DEFAULT_SLIPPAGE_TOLERANCE: f64 = 0.25;

/// Result of profit simulation — either a confirmed opportunity or a rejection.
#[derive(Debug)]
pub enum SimulationResult {
    /// Route is profitable after all costs. Ready to submit.
    Profitable {
        route: ArbRoute,
        net_profit_lamports: u64,
        /// Tip amount (same tip sent to each relay independently)
        tip_lamports: u64,
        /// Profit after tip
        final_profit_lamports: u64,
        /// Minimum final output the arb must produce (input + slippage-adjusted profit)
        min_final_output: u64,
    },
    /// Route is not profitable. Reason provided for logging.
    Unprofitable {
        reason: String,
    },
}

impl ProfitSimulator {
    pub fn new(state_cache: StateCache, tip_fraction: f64, min_profit_lamports: u64, min_tip_lamports: u64) -> Self {
        Self {
            state_cache, tip_fraction, min_profit_lamports, min_tip_lamports,
            slippage_tolerance: DEFAULT_SLIPPAGE_TOLERANCE,
            tip_floor_cache: None,
        }
    }

    /// Set slippage tolerance (0.0 - 1.0). Default is 0.25 (25%).
    pub fn with_slippage_tolerance(mut self, tolerance: f64) -> Self {
        self.slippage_tolerance = tolerance;
        self
    }

    /// Attach a dynamic tip floor cache (from Jito REST API).
    /// When available, the dynamic floor overrides `min_tip_lamports`.
    pub fn with_tip_floor(mut self, cache: TipFloorCache) -> Self {
        self.tip_floor_cache = Some(cache);
        self
    }

    /// Get the effective minimum tip: max of static config floor and dynamic Jito floor.
    fn effective_min_tip(&self) -> u64 {
        let dynamic_floor = self.tip_floor_cache
            .as_ref()
            .and_then(|c| c.get_floor_lamports())
            .unwrap_or(0);
        self.min_tip_lamports.max(dynamic_floor)
    }

    /// Run full simulation on a candidate route.
    ///
    /// This re-simulates with the freshest cached state and applies
    /// all cost deductions before making the go/no-go call.
    pub fn simulate(&self, route: &ArbRoute) -> SimulationResult {
        // Step 1: Re-read pool states from cache (no TTL enforcement).
        // The route calculator already found these pools — re-read the latest
        // cached state for profit estimation. On-chain arb-guard + min_amount_out
        // are the real safety gates, not cache TTL.
        let fresh_states: Vec<_> = route
            .hops
            .iter()
            .map(|hop| self.state_cache.get_any(&hop.pool_address))
            .collect();

        if fresh_states.iter().any(|s| s.is_none()) {
            return SimulationResult::Unprofitable {
                reason: "Pool not found in cache".to_string(),
            };
        }

        let fresh_states: Vec<_> = fresh_states.into_iter().map(|s| s.unwrap()).collect();

        // Step 2: Re-simulate with fresh state, collecting per-hop outputs
        let mut current_amount = route.input_amount;
        let mut fresh_hop_outputs: Vec<u64> = Vec::with_capacity(route.hops.len());

        for (hop, pool) in route.hops.iter().zip(fresh_states.iter()) {
            let a_to_b = match pool.is_a_to_b(&hop.input_mint) {
                Some(dir) => dir,
                None => {
                    return SimulationResult::Unprofitable {
                        reason: format!(
                            "Token {} not found in pool {}",
                            hop.input_mint, pool.address
                        ),
                    };
                }
            };

            // Use bin-by-bin / multi-tick quoting when cache data is available
            let bin_arrays = self.state_cache.get_bin_arrays(&hop.pool_address);
            let tick_arrays = self.state_cache.get_tick_arrays(&hop.pool_address);
            current_amount = match pool.get_output_amount_with_cache(
                current_amount,
                a_to_b,
                bin_arrays.as_deref(),
                tick_arrays.as_deref(),
            ) {
                Some(out) if out > 0 => out,
                _ => {
                    return SimulationResult::Unprofitable {
                        reason: format!(
                            "Zero output on hop {} → {} via {}",
                            hop.input_mint, hop.output_mint, pool.address
                        ),
                    };
                }
            };

            fresh_hop_outputs.push(current_amount);
        }

        // Step 3: Calculate profit (use i128 to avoid overflow with large u64 amounts)
        let gross_profit = (current_amount as i128) - (route.input_amount as i128);

        if gross_profit <= 0 {
            return SimulationResult::Unprofitable {
                reason: format!(
                    "Not profitable: input={} output={} loss={}",
                    route.input_amount, current_amount, -gross_profit
                ),
            };
        }

        let gross_profit_u64 = gross_profit as u64;

        // Sanity cap: any single arb showing > 1 SOL net profit is almost
        // Sanity cap: any route showing > 10 SOL profit is almost certainly
        // an approximation artifact. Arb-guard catches these on-chain, but
        // rejecting them here avoids wasting relay submissions.
        const MAX_SANE_PROFIT: u64 = 10_000_000_000; // 10 SOL
        if gross_profit_u64 > MAX_SANE_PROFIT {
            return SimulationResult::Unprofitable {
                reason: format!(
                    "sanity cap: net profit {} lamports > 10 SOL, likely stale state",
                    gross_profit_u64
                ),
            };
        }

        // Step 4: Apply slippage tolerance to gross profit.
        // E.g. with 25% slippage, plan as if we'll only realize 75% of estimated profit.
        let slippage_adjusted_profit = (gross_profit_u64 as f64 * (1.0 - self.slippage_tolerance)) as u64;

        // Step 5: Smart tip calculation (based on slippage-adjusted profit)
        let fraction_tip = (slippage_adjusted_profit as f64 * self.tip_fraction) as u64;
        let tip_lamports = fraction_tip.max(self.effective_min_tip());

        // Step 6: Reject if tip would exceed slippage-adjusted profit
        if tip_lamports >= slippage_adjusted_profit {
            return SimulationResult::Unprofitable {
                reason: format!(
                    "Tip ({}) >= slippage-adjusted profit ({}), skip",
                    tip_lamports, slippage_adjusted_profit
                ),
            };
        }

        // Step 7: Final profit after tip (based on slippage-adjusted amount)
        let final_profit = slippage_adjusted_profit.saturating_sub(tip_lamports);

        // Step 8: Check minimum threshold
        if final_profit < self.min_profit_lamports {
            return SimulationResult::Unprofitable {
                reason: format!(
                    "Below minimum: final_profit={} < min={}",
                    final_profit, self.min_profit_lamports
                ),
            };
        }

        // Step 9: min_final_output = input + slippage-adjusted profit
        // This is what arb-guard enforces on-chain. More aggressive than just
        // input_amount (break-even), but accounts for 25% slippage.
        let min_final_output = route.input_amount + slippage_adjusted_profit;

        // Step 10: Reconstruct route with fresh estimates and fresh hop outputs
        let mut fresh_route = route.clone();
        fresh_route.estimated_profit = gross_profit as i64;
        fresh_route.estimated_profit_lamports = gross_profit_u64;
        for (hop, &fresh_output) in fresh_route.hops.iter_mut().zip(fresh_hop_outputs.iter()) {
            hop.estimated_output = fresh_output;
        }

        debug!(
            "Profitable route: {} hops, gross={}, adj_profit={}, tip={}, net={}, min_out={}",
            fresh_route.hop_count(), gross_profit_u64, slippage_adjusted_profit,
            tip_lamports, final_profit, min_final_output
        );

        SimulationResult::Profitable {
            route: fresh_route,
            net_profit_lamports: gross_profit_u64,
            tip_lamports,
            final_profit_lamports: final_profit,
            min_final_output,
        }
    }
}
