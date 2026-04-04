use solana_sdk::pubkey::Pubkey;
use tracing::debug;

use crate::router::pool::{ArbRoute, DexType, DetectedSwap, RouteHop};
use crate::state::StateCache;

/// Finds profitable circular arbitrage routes after a detected swap.
///
/// The strategy:
/// 1. A large swap on Pool A moves the price of Token X
/// 2. Token X is now mispriced on Pool A relative to Pool B, C, etc.
/// 3. We find a circular path that exploits this: buy cheap on A → sell expensive on B → back to start
///
/// Speed matters enormously here. We pre-index all token→pool mappings
/// in the StateCache so route discovery is O(1) lookups, not O(n) scans.
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

    /// Find all profitable circular routes that can be executed as a backrun
    /// after the given detected swap.
    ///
    /// Returns routes sorted by estimated profit (highest first).
    pub fn find_routes(&self, swap: &DetectedSwap) -> Vec<ArbRoute> {
        let mut routes = Vec::new();

        // Get the pool state for the swapped pool
        let _pool_state = match self.state_cache.get_any(&swap.pool_address) {
            Some(s) => s,
            None => {
                debug!("No cached state for pool {}", swap.pool_address);
                return routes;
            }
        };

        // The swap disturbs the price on this pool.
        // We look for circular paths starting from the output token.
        //
        // Example: User swaps SOL → USDC on Raydium
        // After swap, SOL is relatively cheaper on Raydium (more SOL in pool)
        // We look for: buy SOL cheap on Raydium → sell SOL on Orca → back to USDC
        //
        // Base token: we use the OUTPUT token of the detected swap as our starting point
        // because the price dislocation creates opportunity in the output direction.
        let base_mint = swap.output_mint;

        // Find 2-hop routes: base → X → base (through different pools)
        self.find_2_hop_routes(&base_mint, &swap.pool_address, &mut routes);

        // Find 3-hop routes: base → X → Y → base
        if self.max_hops >= 3 {
            self.find_3_hop_routes(&base_mint, &swap.pool_address, &mut routes);
        }

        // ALWAYS search with SOL as base — we hold SOL, so SOL→X→SOL routes
        // are always executable. Any pool state change could create a SOL arb.
        let sol = crate::config::sol_mint();
        if base_mint != sol {
            // Search SOL-base routes through ALL pools (not just trigger pool)
            let sol_trigger = crate::router::pool::DetectedSwap {
                dex_type: swap.dex_type,
                pool_address: swap.pool_address,
                input_mint: sol,
                output_mint: swap.input_mint, // use the other token
                amount: None,
                observed_slot: swap.observed_slot,
            };
            self.find_2_hop_routes(&sol, &sol_trigger.pool_address, &mut routes);
            if self.max_hops >= 3 {
                self.find_3_hop_routes(&sol, &sol_trigger.pool_address, &mut routes);
            }
        }

        // Sort by estimated profit descending
        routes.sort_by(|a, b| b.estimated_profit.cmp(&a.estimated_profit));

        routes
    }

    /// Find 2-hop circular routes: base → other → base
    ///
    /// This looks for pairs of pools where:
    /// Pool 1: trades base/other
    /// Pool 2: also trades base/other (different DEX or different pool)
    fn find_2_hop_routes(
        &self,
        base_mint: &Pubkey,
        _trigger_pool: &Pubkey,
        routes: &mut Vec<ArbRoute>,
    ) {
        // Get all pools that contain our base token
        let base_pools = self.state_cache.pools_for_token(base_mint);

        for pool_a_addr in &base_pools {
            let pool_a = match self.state_cache.get_any(pool_a_addr) {
                Some(s) => s,
                None => continue,
            };

            let other_mint = match pool_a.other_token(base_mint) {
                Some(m) => m,
                None => continue,
            };

            // Find pools that also trade base/other but are different pools
            let return_pools = self.state_cache.pools_for_pair(base_mint, &other_mint);

            for pool_b_addr in &return_pools {
                // Skip same pool
                if pool_b_addr == pool_a_addr {
                    continue;
                }

                let pool_b = match self.state_cache.get_any(pool_b_addr) {
                    Some(s) => s,
                    None => continue,
                };

                // Simulate the 2-hop route with a small test amount
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
                    }
                }
            }
        }
    }

    /// Find 3-hop circular routes: base → mid1 → mid2 → base
    fn find_3_hop_routes(
        &self,
        base_mint: &Pubkey,
        _trigger_pool: &Pubkey,
        routes: &mut Vec<ArbRoute>,
    ) {
        let base_pools = self.state_cache.pools_for_token(base_mint);

        for pool_a_addr in &base_pools {
            let pool_a = match self.state_cache.get_any(pool_a_addr) {
                Some(s) => s,
                None => continue,
            };

            let mid1_mint = match pool_a.other_token(base_mint) {
                Some(m) => m,
                None => continue,
            };

            // Find pools containing mid1
            let mid1_pools = self.state_cache.pools_for_token(&mid1_mint);

            for pool_b_addr in &mid1_pools {
                if pool_b_addr == pool_a_addr {
                    continue;
                }

                let pool_b = match self.state_cache.get_any(pool_b_addr) {
                    Some(s) => s,
                    None => continue,
                };

                let mid2_mint = match pool_b.other_token(&mid1_mint) {
                    Some(m) => m,
                    None => continue,
                };

                // Skip if mid2 == base (that's a 2-hop, already covered)
                if mid2_mint == *base_mint {
                    continue;
                }

                // Find pools that trade mid2/base to close the circle
                let return_pools = self.state_cache.pools_for_pair(&mid2_mint, base_mint);

                for pool_c_addr in &return_pools {
                    if pool_c_addr == pool_a_addr || pool_c_addr == pool_b_addr {
                        continue;
                    }

                    let pool_c = match self.state_cache.get_any(pool_c_addr) {
                        Some(s) => s,
                        None => continue,
                    };

                    // Use a conservative test amount for 3-hop
                    let test_amount = 1_000_000u64; // 0.001 SOL worth in lamports

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
        // Hop 1: base → other on pool_a
        let a_to_b_a = pool_a.is_a_to_b(base_mint)?;
        let bins_a = self.state_cache.get_bin_arrays(&pool_a.address);
        let mid_amount = pool_a.get_output_amount_with_bins(
            input_amount,
            a_to_b_a,
            bins_a.as_deref(),
        )?;

        if mid_amount == 0 {
            return None;
        }

        // Hop 2: other → base on pool_b
        let a_to_b_b = pool_b.is_a_to_b(other_mint)?;
        let bins_b = self.state_cache.get_bin_arrays(&pool_b.address);
        let final_amount = pool_b.get_output_amount_with_bins(
            mid_amount,
            a_to_b_b,
            bins_b.as_deref(),
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
        // Hop 1: base → mid1
        let a_to_b_a = pool_a.is_a_to_b(base_mint)?;
        let bins_a = self.state_cache.get_bin_arrays(&pool_a.address);
        let amount_1 = pool_a.get_output_amount_with_bins(
            input_amount, a_to_b_a, bins_a.as_deref(),
        )?;
        if amount_1 == 0 { return None; }

        // Hop 2: mid1 → mid2
        let a_to_b_b = pool_b.is_a_to_b(mid1_mint)?;
        let bins_b = self.state_cache.get_bin_arrays(&pool_b.address);
        let amount_2 = pool_b.get_output_amount_with_bins(
            amount_1, a_to_b_b, bins_b.as_deref(),
        )?;
        if amount_2 == 0 { return None; }

        // Hop 3: mid2 → base
        let a_to_b_c = pool_c.is_a_to_b(mid2_mint)?;
        let bins_c = self.state_cache.get_bin_arrays(&pool_c.address);
        let final_amount = pool_c.get_output_amount_with_bins(
            amount_2, a_to_b_c, bins_c.as_deref(),
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
    ///
    /// For constant-product pools, optimal input can be derived analytically.
    /// For now, we use a conservative fixed fraction of the smaller pool's reserves.
    fn calculate_optimal_input(
        &self,
        pool_a: &crate::router::pool::PoolState,
        pool_b: &crate::router::pool::PoolState,
        base_mint: &Pubkey,
    ) -> u64 {
        // Use 1% of the smaller pool's base-side reserve as starting input
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
        // 1% of smaller reserve, minimum 10000 lamports
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
    ))
}
