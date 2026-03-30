use tracing::debug;

use crate::router::pool::ArbRoute;
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
}

/// Result of profit simulation — either a confirmed opportunity or a rejection.
#[derive(Debug)]
pub enum SimulationResult {
    /// Route is profitable after all costs. Ready to submit.
    Profitable {
        route: ArbRoute,
        net_profit_lamports: u64,
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
    pub fn new(state_cache: StateCache, tip_fraction: f64, min_profit_lamports: u64) -> Self {
        Self {
            state_cache,
            tip_fraction,
            min_profit_lamports,
        }
    }

    /// Run full simulation on a candidate route.
    ///
    /// This re-simulates with the freshest cached state and applies
    /// all cost deductions before making the go/no-go call.
    pub fn simulate(&self, route: &ArbRoute) -> SimulationResult {
        // Step 1: Re-read pool states (freshest from cache)
        let fresh_states: Vec<_> = route
            .hops
            .iter()
            .map(|hop| self.state_cache.get(&hop.pool_address))
            .collect();

        // If any pool state is stale (expired TTL), abort
        if fresh_states.iter().any(|s| s.is_none()) {
            return SimulationResult::Unprofitable {
                reason: "Stale pool state — one or more pools expired from cache".to_string(),
            };
        }

        let fresh_states: Vec<_> = fresh_states.into_iter().map(|s| s.unwrap()).collect();

        // Step 2: Re-simulate with fresh state
        let mut current_amount = route.input_amount;

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

            current_amount = match pool.get_output_amount(current_amount, a_to_b) {
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
        }

        // Step 3: Calculate profit
        let gross_profit = current_amount as i64 - route.input_amount as i64;

        if gross_profit <= 0 {
            return SimulationResult::Unprofitable {
                reason: format!(
                    "Not profitable: input={} output={} loss={}",
                    route.input_amount, current_amount, -gross_profit
                ),
            };
        }

        let gross_profit_u64 = gross_profit as u64;

        // Step 4: Calculate Jito tip
        let tip_lamports = (gross_profit_u64 as f64 * self.tip_fraction) as u64;

        // Step 5: Final profit after tip
        let final_profit = gross_profit_u64.saturating_sub(tip_lamports);

        // Step 6: Check minimum threshold
        if final_profit < self.min_profit_lamports {
            return SimulationResult::Unprofitable {
                reason: format!(
                    "Below minimum: final_profit={} < min={}",
                    final_profit, self.min_profit_lamports
                ),
            };
        }

        // Step 7: Reconstruct route with fresh estimates
        let mut fresh_route = route.clone();
        fresh_route.estimated_profit = gross_profit;
        fresh_route.estimated_profit_lamports = gross_profit_u64;

        debug!(
            "Profitable route: {} hops, gross={}, tip={}, net={}",
            fresh_route.hop_count(),
            gross_profit_u64,
            tip_lamports,
            final_profit
        );

        SimulationResult::Profitable {
            route: fresh_route,
            net_profit_lamports: gross_profit_u64,
            tip_lamports,
            final_profit_lamports: final_profit,
        }
    }
}
