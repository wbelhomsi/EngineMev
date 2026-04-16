use solana_sdk::pubkey::Pubkey;

use crate::router::pool::{ArbRoute, DexType, DetectedSwap, RouteHop};
use crate::state::StateCache;

/// Maximum pools to consider per token during route search.
/// Limits the combinatorial explosion while keeping the best liquidity pools.
const MAX_POOLS_PER_TOKEN: usize = 20;

/// Early-exit: stop searching once we have this many profitable routes.
/// The first few are almost always the best (sorted by reserve-based heuristic).
const EARLY_EXIT_ROUTES: usize = 5;

/// Finds profitable circular arbitrage routes after a detected swap.
///
/// Speed-optimized: caps pool iteration, skips 3-hop by default,
/// early-exits on sufficient candidates.
pub struct RouteCalculator {
    state_cache: StateCache,
    max_hops: usize,
}

impl RouteCalculator {
    pub fn new(state_cache: StateCache, max_hops: usize) -> Self {
        Self {
            state_cache,
            max_hops,
        }
    }

    /// Find all profitable circular routes starting from a single base mint.
    /// Called once per event with SOL as base (the only token we hold).
    pub fn find_routes_for_base(
        &self,
        base_mint: &Pubkey,
        trigger_pool: &Pubkey,
    ) -> Vec<ArbRoute> {
        let mut routes = Vec::with_capacity(EARLY_EXIT_ROUTES);

        self.find_2_hop_routes(base_mint, trigger_pool, &mut routes);

        if self.max_hops >= 3 && routes.len() < EARLY_EXIT_ROUTES {
            self.find_3_hop_routes(base_mint, trigger_pool, &mut routes);
        }

        routes.sort_unstable_by(|a, b| b.estimated_profit.cmp(&a.estimated_profit));
        routes
    }

    /// Legacy entry point — calls find_routes_for_base with SOL as base.
    pub fn find_routes(&self, swap: &DetectedSwap) -> Vec<ArbRoute> {
        let sol = crate::config::sol_mint();
        self.find_routes_for_base(&sol, &swap.pool_address)
    }

    /// Find 2-hop circular routes: base → other → base
    fn find_2_hop_routes(
        &self,
        base_mint: &Pubkey,
        _trigger_pool: &Pubkey,
        routes: &mut Vec<ArbRoute>,
    ) {
        let base_pools = self.state_cache.pools_for_token(base_mint);

        // Cap iteration to the first N pools (DashMap order is arbitrary but stable
        // within a single read — good enough for our purposes)
        let pool_limit = base_pools.len().min(MAX_POOLS_PER_TOKEN);

        for pool_a_addr in &base_pools[..pool_limit] {
            if routes.len() >= EARLY_EXIT_ROUTES {
                return;
            }

            let pool_a = match self.state_cache.get_any(pool_a_addr) {
                Some(s) => s,
                None => continue,
            };

            let other_mint = match pool_a.other_token(base_mint) {
                Some(m) => m,
                None => continue,
            };

            let return_pools = self.state_cache.pools_for_pair(base_mint, &other_mint);

            for pool_b_addr in &return_pools {
                if pool_b_addr == pool_a_addr {
                    continue;
                }

                let pool_b = match self.state_cache.get_any(pool_b_addr) {
                    Some(s) => s,
                    None => continue,
                };

                let test_amount = self.calculate_optimal_input(&pool_a, &pool_b, base_mint);

                if let Some(route) = self.simulate_2_hop(
                    base_mint,
                    &other_mint,
                    &pool_a,
                    &pool_b,
                    test_amount,
                ) {
                    if route.is_profitable() {
                        routes.push(route);
                        if routes.len() >= EARLY_EXIT_ROUTES {
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Find 3-hop circular routes: base → mid1 → mid2 → base
    /// Only runs when 2-hop didn't find enough candidates.
    fn find_3_hop_routes(
        &self,
        base_mint: &Pubkey,
        _trigger_pool: &Pubkey,
        routes: &mut Vec<ArbRoute>,
    ) {
        let base_pools = self.state_cache.pools_for_token(base_mint);
        let pool_limit = base_pools.len().min(MAX_POOLS_PER_TOKEN);

        for pool_a_addr in &base_pools[..pool_limit] {
            if routes.len() >= EARLY_EXIT_ROUTES {
                return;
            }

            let pool_a = match self.state_cache.get_any(pool_a_addr) {
                Some(s) => s,
                None => continue,
            };

            let mid1_mint = match pool_a.other_token(base_mint) {
                Some(m) => m,
                None => continue,
            };

            let mid1_pools = self.state_cache.pools_for_token(&mid1_mint);
            let mid1_limit = mid1_pools.len().min(MAX_POOLS_PER_TOKEN);

            for pool_b_addr in &mid1_pools[..mid1_limit] {
                if pool_b_addr == pool_a_addr {
                    continue;
                }
                if routes.len() >= EARLY_EXIT_ROUTES {
                    return;
                }

                let pool_b = match self.state_cache.get_any(pool_b_addr) {
                    Some(s) => s,
                    None => continue,
                };

                let mid2_mint = match pool_b.other_token(&mid1_mint) {
                    Some(m) => m,
                    None => continue,
                };

                if mid2_mint == *base_mint {
                    continue;
                }

                let return_pools = self.state_cache.pools_for_pair(&mid2_mint, base_mint);

                for pool_c_addr in &return_pools {
                    if pool_c_addr == pool_a_addr || pool_c_addr == pool_b_addr {
                        continue;
                    }

                    let pool_c = match self.state_cache.get_any(pool_c_addr) {
                        Some(s) => s,
                        None => continue,
                    };

                    let test_amount = 1_000_000u64; // 0.001 SOL

                    if let Some(route) = self.simulate_3_hop(
                        base_mint,
                        &mid1_mint,
                        &mid2_mint,
                        &pool_a,
                        &pool_b,
                        &pool_c,
                        test_amount,
                    ) {
                        if route.is_profitable() {
                            routes.push(route);
                            if routes.len() >= EARLY_EXIT_ROUTES {
                                return;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Simulate a 2-hop route and return the ArbRoute with profit estimation.
    fn simulate_2_hop(
        &self,
        base_mint: &Pubkey,
        other_mint: &Pubkey,
        pool_a: &crate::router::pool::PoolState,
        pool_b: &crate::router::pool::PoolState,
        input_amount: u64,
    ) -> Option<ArbRoute> {
        let a_to_b_a = pool_a.is_a_to_b(base_mint)?;
        let bins_a = self.state_cache.get_bin_arrays(&pool_a.address);
        let ticks_a = self.state_cache.get_tick_arrays(&pool_a.address);
        let mid_amount = pool_a.get_output_amount_with_cache(
            input_amount,
            a_to_b_a,
            bins_a.as_deref(),
            ticks_a.as_deref(),
        )?;

        if mid_amount == 0 {
            return None;
        }

        let a_to_b_b = pool_b.is_a_to_b(other_mint)?;
        let bins_b = self.state_cache.get_bin_arrays(&pool_b.address);
        let ticks_b = self.state_cache.get_tick_arrays(&pool_b.address);
        let final_amount = pool_b.get_output_amount_with_cache(
            mid_amount,
            a_to_b_b,
            bins_b.as_deref(),
            ticks_b.as_deref(),
        )?;

        let profit = (final_amount as i128) - (input_amount as i128);

        Some(ArbRoute {
            hops: vec![
                RouteHop {
                    pool_address: pool_a.address,
                    dex_type: pool_a.dex_type,
                    input_mint: *base_mint,
                    output_mint: *other_mint,
                    estimated_output: mid_amount,
                },
                RouteHop {
                    pool_address: pool_b.address,
                    dex_type: pool_b.dex_type,
                    input_mint: *other_mint,
                    output_mint: *base_mint,
                    estimated_output: final_amount,
                },
            ],
            base_mint: *base_mint,
            input_amount,
            estimated_profit: profit as i64,
            estimated_profit_lamports: if profit > 0 { profit as u64 } else { 0 },
        })
    }

    /// Simulate a 3-hop route.
    fn simulate_3_hop(
        &self,
        base_mint: &Pubkey,
        mid1_mint: &Pubkey,
        mid2_mint: &Pubkey,
        pool_a: &crate::router::pool::PoolState,
        pool_b: &crate::router::pool::PoolState,
        pool_c: &crate::router::pool::PoolState,
        input_amount: u64,
    ) -> Option<ArbRoute> {
        let a_to_b_a = pool_a.is_a_to_b(base_mint)?;
        let bins_a = self.state_cache.get_bin_arrays(&pool_a.address);
        let ticks_a = self.state_cache.get_tick_arrays(&pool_a.address);
        let amount_1 = pool_a.get_output_amount_with_cache(
            input_amount, a_to_b_a, bins_a.as_deref(), ticks_a.as_deref(),
        )?;
        if amount_1 == 0 { return None; }

        let a_to_b_b = pool_b.is_a_to_b(mid1_mint)?;
        let bins_b = self.state_cache.get_bin_arrays(&pool_b.address);
        let ticks_b = self.state_cache.get_tick_arrays(&pool_b.address);
        let amount_2 = pool_b.get_output_amount_with_cache(
            amount_1, a_to_b_b, bins_b.as_deref(), ticks_b.as_deref(),
        )?;
        if amount_2 == 0 { return None; }

        let a_to_b_c = pool_c.is_a_to_b(mid2_mint)?;
        let bins_c = self.state_cache.get_bin_arrays(&pool_c.address);
        let ticks_c = self.state_cache.get_tick_arrays(&pool_c.address);
        let final_amount = pool_c.get_output_amount_with_cache(
            amount_2, a_to_b_c, bins_c.as_deref(), ticks_c.as_deref(),
        )?;

        let profit = (final_amount as i128) - (input_amount as i128);

        Some(ArbRoute {
            hops: vec![
                RouteHop {
                    pool_address: pool_a.address,
                    dex_type: pool_a.dex_type,
                    input_mint: *base_mint,
                    output_mint: *mid1_mint,
                    estimated_output: amount_1,
                },
                RouteHop {
                    pool_address: pool_b.address,
                    dex_type: pool_b.dex_type,
                    input_mint: *mid1_mint,
                    output_mint: *mid2_mint,
                    estimated_output: amount_2,
                },
                RouteHop {
                    pool_address: pool_c.address,
                    dex_type: pool_c.dex_type,
                    input_mint: *mid2_mint,
                    output_mint: *base_mint,
                    estimated_output: final_amount,
                },
            ],
            base_mint: *base_mint,
            input_amount,
            estimated_profit: profit as i64,
            estimated_profit_lamports: if profit > 0 { profit as u64 } else { 0 },
        })
    }

    /// Estimate optimal input amount for a 2-hop arb.
    fn calculate_optimal_input(
        &self,
        pool_a: &crate::router::pool::PoolState,
        pool_b: &crate::router::pool::PoolState,
        base_mint: &Pubkey,
    ) -> u64 {
        let reserve_a = if pool_a.token_a_mint == *base_mint {
            pool_a.token_a_reserve
        } else {
            pool_a.token_b_reserve
        };

        let reserve_b = if pool_b.token_a_mint == *base_mint {
            pool_b.token_a_reserve
        } else {
            pool_b.token_b_reserve
        };

        let min_reserve = reserve_a.min(reserve_b);
        (min_reserve / 100).max(10_000)
    }
}

/// Check if all hops in a route use DEXes with real swap IX builders.
pub fn can_submit_route(route: &ArbRoute) -> bool {
    route.hops.iter().all(|hop| matches!(
        hop.dex_type,
        DexType::RaydiumAmm
        | DexType::RaydiumCp
        | DexType::RaydiumClmm
        | DexType::OrcaWhirlpool
        | DexType::MeteoraDlmm
        | DexType::MeteoraDammV2
        | DexType::SanctumInfinity
        | DexType::Phoenix
        | DexType::Manifest
        | DexType::PumpSwap
    ))
}
