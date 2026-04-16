//! Divergence detector: compares CEX prices vs on-chain pool prices and
//! constructs a CexDexRoute for the best opportunity.

use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use crate::addresses;
use crate::cexdex::inventory::Inventory;
use crate::cexdex::price_store::PriceStore;
use crate::cexdex::route::{ArbDirection, CexDexRoute};
use crate::cexdex::units::{
    atoms_to_usdc, lamports_to_sol, sol_to_lamports, spread_bps, usdc_to_atoms,
};
use crate::feed::PriceSnapshot;
use crate::router::pool::PoolState;

/// Configuration knobs for the divergence detector.
#[derive(Debug, Clone)]
pub struct DetectorConfig {
    /// Minimum price divergence (in basis points) to consider an opportunity.
    pub min_spread_bps: u64,
    /// Minimum expected profit in USD (after slippage discount) to emit a route.
    pub min_profit_usd: f64,
    /// Hard cap on trade size in SOL (both directions).
    pub max_trade_size_sol: f64,
    /// Maximum age of the CEX price snapshot before we refuse to act.
    pub cex_staleness_ms: u64,
    /// Fraction of gross profit to discount for slippage (0.25 = 25%).
    pub slippage_tolerance: f64,
}

/// Core divergence detector.
///
/// Checks all monitored pools against the CEX price and returns the single
/// best `CexDexRoute` (highest expected_profit_usd), or `None` if no
/// profitable opportunity exists.
pub struct Detector {
    store: PriceStore,
    inventory: Inventory,
    /// Monitored pools: (dex_type, pool_address).
    pools: Vec<(crate::router::pool::DexType, Pubkey)>,
    config: DetectorConfig,
}

fn usdc_mint() -> Pubkey {
    Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap()
}

impl Detector {
    pub fn new(
        store: PriceStore,
        inventory: Inventory,
        pools: Vec<(crate::router::pool::DexType, Pubkey)>,
        config: DetectorConfig,
    ) -> Self {
        Self { store, inventory, pools, config }
    }

    /// Check all monitored pools against the current CEX price.
    ///
    /// Returns the best (highest profit) `CexDexRoute`, or `None` if:
    /// - CEX price is stale
    /// - No pool shows sufficient divergence
    /// - Inventory hard cap blocks all directions
    /// - Adjusted profit is below `min_profit_usd`
    pub fn check_all(&self) -> Option<CexDexRoute> {
        // Gate 1: reject stale CEX data
        if self.store.is_stale("SOLUSDC", self.config.cex_staleness_ms) {
            return None;
        }
        let cex = self.store.get_cex("SOLUSDC")?;

        let mut best: Option<CexDexRoute> = None;

        for &(_dex, pool_addr) in &self.pools {
            let pool = match self.store.pools.get_any(&pool_addr) {
                Some(p) => p,
                None => continue,
            };

            for direction in [ArbDirection::BuyOnDex, ArbDirection::SellOnDex] {
                // Gate 2: inventory hard cap
                if !self.inventory.allows_direction(direction) {
                    continue;
                }

                let route = match self.try_route(&pool, direction, &cex) {
                    Some(r) => r,
                    None => continue,
                };

                // Gate 3: minimum profit, scaled by inventory skew multiplier
                let required_profit = self.config.min_profit_usd
                    * self.inventory.profit_multiplier(direction);
                if route.expected_profit_usd < required_profit {
                    continue;
                }

                // Keep highest-profit route
                let is_better = match &best {
                    None => true,
                    Some(b) => route.expected_profit_usd > b.expected_profit_usd,
                };
                if is_better {
                    best = Some(route);
                }
            }
        }

        best
    }

    /// Attempt to build a `CexDexRoute` for a single pool + direction.
    ///
    /// Returns `None` if:
    /// - Pool is not a SOL/USDC pair
    /// - Reserves are zero
    /// - Divergence is below `min_spread_bps`
    /// - Trade size would be negligible (< 0.001 SOL)
    /// - On-chain quote returns zero
    /// - Gross profit is non-positive after slippage discount
    fn try_route(
        &self,
        pool: &PoolState,
        direction: ArbDirection,
        cex: &PriceSnapshot,
    ) -> Option<CexDexRoute> {
        let wsol = addresses::WSOL;
        let usdc = usdc_mint();

        // Determine which side is SOL and which is USDC
        let (sol_is_a, sol_reserve, usdc_reserve) =
            if pool.token_a_mint == wsol && pool.token_b_mint == usdc {
                (true, pool.token_a_reserve, pool.token_b_reserve)
            } else if pool.token_b_mint == wsol && pool.token_a_mint == usdc {
                (false, pool.token_b_reserve, pool.token_a_reserve)
            } else {
                return None; // not a SOL/USDC pool
            };

        if sol_reserve == 0 || usdc_reserve == 0 {
            return None;
        }

        // DEX spot price: USDC per SOL
        let dex_spot = atoms_to_usdc(usdc_reserve) / lamports_to_sol(sol_reserve);

        // Check divergence direction and magnitude
        let (reference_price, edge_bps) = match direction {
            ArbDirection::BuyOnDex => {
                // Profitable only if DEX is cheaper than CEX bid
                if dex_spot >= cex.best_bid_usd {
                    return None;
                }
                (cex.best_bid_usd, spread_bps(cex.best_bid_usd, dex_spot))
            }
            ArbDirection::SellOnDex => {
                // Profitable only if DEX is more expensive than CEX ask
                if dex_spot <= cex.best_ask_usd {
                    return None;
                }
                (cex.best_ask_usd, spread_bps(cex.best_ask_usd, dex_spot))
            }
        };

        if edge_bps < self.config.min_spread_bps {
            return None;
        }

        // Size the trade: bounded by available inventory, pool liquidity cap, and max_trade_size_sol.
        //
        // Pool liquidity cap: limit to 1% of the input-side reserve to avoid excessive
        // price impact that would erase the arb edge. At ~2.7% edge, even 1% of pool
        // size yields meaningful profit while keeping price impact manageable.
        let max_sol = self.config.max_trade_size_sol;
        let pool_liquidity_cap_sol = {
            // 1% of the smaller pool side, expressed as SOL
            let pool_usdc = atoms_to_usdc(usdc_reserve);
            let pool_sol = lamports_to_sol(sol_reserve);
            let sol_from_usdc_cap = (pool_usdc * 0.01) / reference_price;
            let sol_cap = pool_sol * 0.01;
            sol_from_usdc_cap.min(sol_cap)
        };
        let trade_sol = match direction {
            ArbDirection::BuyOnDex => {
                // We spend USDC to buy SOL — size in SOL equivalent
                let usdc_available = self.inventory.usdc_atoms_available();
                let usdc_cap = atoms_to_usdc(usdc_available);
                let sol_from_usdc = usdc_cap / reference_price;
                sol_from_usdc.min(max_sol).min(pool_liquidity_cap_sol)
            }
            ArbDirection::SellOnDex => {
                // We spend SOL to get USDC
                let sol_available = lamports_to_sol(self.inventory.sol_lamports_available());
                sol_available.min(max_sol).min(pool_liquidity_cap_sol)
            }
        };

        if trade_sol < 0.001 {
            return None; // negligible trade
        }

        // Build concrete (input_amount, input_mint, output_mint, a_to_b)
        let (input_amount, input_mint, output_mint, a_to_b) = match direction {
            ArbDirection::BuyOnDex => {
                // Spend USDC, receive SOL
                let usdc_to_spend = trade_sol * reference_price;
                let input = usdc_to_atoms(usdc_to_spend);
                // If SOL is token_a, then USDC is token_b → b_to_a → a_to_b = false
                let a_to_b = !sol_is_a;
                (input, usdc, wsol, a_to_b)
            }
            ArbDirection::SellOnDex => {
                // Spend SOL, receive USDC
                let input = sol_to_lamports(trade_sol);
                // If SOL is token_a → a_to_b = true
                let a_to_b = sol_is_a;
                (input, wsol, usdc, a_to_b)
            }
        };

        // Get actual on-chain output quote
        let output = pool.get_output_amount_with_cache(
            input_amount,
            a_to_b,
            self.store.pools.get_bin_arrays(&pool.address).as_deref(),
            self.store.pools.get_tick_arrays(&pool.address).as_deref(),
        )?;
        if output == 0 {
            return None;
        }

        // Compute profit in USD
        let (input_usd, output_usd) = match direction {
            ArbDirection::BuyOnDex => {
                // We paid input USDC, received output SOL worth output_sol * bid
                let input_usd = atoms_to_usdc(input_amount);
                let output_usd = lamports_to_sol(output) * cex.best_bid_usd;
                (input_usd, output_usd)
            }
            ArbDirection::SellOnDex => {
                // We paid input SOL worth input_sol * ask, received output USDC
                let input_usd = lamports_to_sol(input_amount) * cex.best_ask_usd;
                let output_usd = atoms_to_usdc(output);
                (input_usd, output_usd)
            }
        };

        let gross_profit_usd = output_usd - input_usd;
        let slippage_discount = 1.0 - self.config.slippage_tolerance;
        let adjusted_profit_usd = gross_profit_usd * slippage_discount;
        if adjusted_profit_usd <= 0.0 {
            return None;
        }

        Some(CexDexRoute {
            pool_address: pool.address,
            dex_type: pool.dex_type,
            direction,
            input_mint,
            output_mint,
            input_amount,
            expected_output: output,
            cex_bid_at_detection: cex.best_bid_usd,
            cex_ask_at_detection: cex.best_ask_usd,
            expected_profit_usd: adjusted_profit_usd,
            observed_slot: pool.last_slot,
        })
    }
}
