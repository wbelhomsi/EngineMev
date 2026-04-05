use tracing::debug;

use crate::router::pool::{ArbRoute, DexType};
use crate::state::StateCache;

/// Final profit simulation before bundle submission.
///
/// The RouteCalculator does fast approximate math to find candidates.
/// The ProfitSimulator does precise validation:
/// - Re-reads freshest pool state from cache
/// - Accounts for exact fees and tick-crossing (CLMM)
/// - Calculates tip amount based on profit
/// - Returns a go/no-go decision
///
/// This is the last gate before we spend money (Jito tip).
/// If simulation says no-go, we drop the opportunity. No partial bets.
pub struct ProfitSimulator {
    state_cache: StateCache,
    tip_fraction: f64,
    min_profit_lamports: u64,
    min_tip_lamports: u64,
}

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
    },
    /// Route is not profitable. Reason provided for logging.
    Unprofitable {
        reason: String,
    },
}

impl ProfitSimulator {
    pub fn new(state_cache: StateCache, tip_fraction: f64, min_profit_lamports: u64, min_tip_lamports: u64) -> Self {
        Self { state_cache, tip_fraction, min_profit_lamports, min_tip_lamports }
    }

    /// Run full simulation on a candidate route.
    ///
    /// This re-simulates with the freshest cached state and applies
    /// all cost deductions before making the go/no-go call.
    pub fn simulate(&self, route: &ArbRoute) -> SimulationResult {
        // Step 1: Re-read pool states from cache with TTL enforcement.
        // Sanctum virtual pools don't get frequent Geyser updates, so use
        // get_any() (no TTL) for them. All other DEXes use get() with TTL
        // to ensure the simulator gates on fresh state.
        let fresh_states: Vec<_> = route
            .hops
            .iter()
            .map(|hop| {
                if hop.dex_type == DexType::SanctumInfinity {
                    self.state_cache.get_any(&hop.pool_address)
                } else {
                    self.state_cache.get(&hop.pool_address)
                }
            })
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

        // Step 4: Smart tip calculation
        // - If profit <= min_tip: skip (can't even cover the tip)
        // - If profit * fraction < min_tip: use min_tip (floor for auction competitiveness)
        // - If profit * fraction >= min_tip: use profit * fraction (normal %)
        let fraction_tip = (gross_profit_u64 as f64 * self.tip_fraction) as u64;
        let tip_lamports = fraction_tip.max(self.min_tip_lamports);

        // Step 5: Reject if tip would exceed or equal profit (can't tip more than we earn)
        if tip_lamports >= gross_profit_u64 {
            return SimulationResult::Unprofitable {
                reason: format!(
                    "Tip ({}) >= gross profit ({}), skip",
                    tip_lamports, gross_profit_u64
                ),
            };
        }

        // Step 6: Final profit after tip
        let final_profit = gross_profit_u64.saturating_sub(tip_lamports);

        // Step 7: Check minimum threshold
        if final_profit < self.min_profit_lamports {
            return SimulationResult::Unprofitable {
                reason: format!(
                    "Below minimum: final_profit={} < min={}",
                    final_profit, self.min_profit_lamports
                ),
            };
        }

        // Step 8: Reconstruct route with fresh estimates and fresh hop outputs
        let mut fresh_route = route.clone();
        fresh_route.estimated_profit = gross_profit as i64;
        fresh_route.estimated_profit_lamports = gross_profit_u64;
        for (hop, &fresh_output) in fresh_route.hops.iter_mut().zip(fresh_hop_outputs.iter()) {
            hop.estimated_output = fresh_output;
        }

        debug!(
            "Profitable route: {} hops, gross={}, tip={}, net={}",
            fresh_route.hop_count(), gross_profit_u64, tip_lamports, final_profit
        );

        SimulationResult::Profitable {
            route: fresh_route,
            net_profit_lamports: gross_profit_u64,
            tip_lamports,
            final_profit_lamports: final_profit,
        }
    }
}
