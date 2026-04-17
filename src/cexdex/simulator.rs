//! CEX-priced profit simulator for CexDexRoute.
//!
//! Re-reads fresh pool state (no TTL — arb-guard gates on-chain), re-quotes at the
//! route's input_amount using current pool reserves, prices the output via the
//! current CEX mid, and computes the worst-case net profit (using max_tip_fraction)
//! and the on-chain `min_final_output` (conservative, slippage-tolerant).
//!
//! The simulator does NOT compute tip_lamports — that is deferred to dispatch time
//! so each relay can use its own tip fraction (nonce fan-out, Task 9).

use tracing::debug;

use crate::cexdex::price_store::PriceStore;
use crate::cexdex::route::{ArbDirection, CexDexRoute};
use crate::cexdex::units::{atoms_to_usdc, lamports_to_sol, sol_to_lamports, usdc_to_atoms};

/// Configuration for the CEX-DEX profit simulator.
#[derive(Debug, Clone)]
pub struct CexDexSimulatorConfig {
    /// Minimum net profit in USD required to proceed with bundle submission.
    pub min_profit_usd: f64,
    /// Discount applied to gross profit before tip calculation (0.25 = 25%).
    pub slippage_tolerance: f64,
    /// Estimated on-chain transaction fee in lamports.
    pub tx_fee_lamports: u64,
    /// Minimum tip in lamports (floor for competitiveness). Used by the binary
    /// at dispatch time when computing per-relay tip (Task 9).
    pub min_tip_lamports: u64,
    /// Highest per-relay tip fraction. The sim uses this to gate the worst case
    /// (the relay paying the max tip) so any dispatch passing the gate is
    /// profitable regardless of which relay lands first.
    pub max_tip_fraction: f64,
}

/// Result of simulating a `CexDexRoute`.
#[derive(Debug)]
pub enum SimulationResult {
    /// Route passes all gates — proceed to bundle building.
    Profitable {
        /// Route with fresh quote + CEX prices written back.
        route: CexDexRoute,
        /// Slippage-adjusted gross profit in SOL. Caller computes per-relay
        /// tip = adjusted_profit_sol * tip_fractions[relay].
        adjusted_profit_sol: f64,
        /// Slippage-adjusted gross profit in USD (convenience; used by stats/logging).
        adjusted_profit_usd: f64,
        /// Worst-case net after tip (using max_tip_fraction) and tx fee.
        /// Sim rejected if this was below min_profit_usd — so every Profitable
        /// return is profitable regardless of which relay lands first.
        net_profit_usd_worst_case: f64,
        /// On-chain arb-guard floor (minimum acceptable output).
        min_final_output: u64,
    },
    /// Route fails a gate — do not build a bundle.
    Unprofitable {
        reason: String,
    },
}

/// Stateless simulator: takes a `PriceStore` reference and a config.
pub struct CexDexSimulator {
    store: PriceStore,
    config: CexDexSimulatorConfig,
}

impl CexDexSimulator {
    pub fn new(store: PriceStore, config: CexDexSimulatorConfig) -> Self {
        Self { store, config }
    }

    /// Simulate a route and return `Profitable` or `Unprofitable`.
    ///
    /// Uses `get_any()` (no TTL) because the on-chain arb-guard enforces
    /// `min_final_output` as the real safety gate.
    pub fn simulate(&self, route: &CexDexRoute) -> SimulationResult {
        // Step 1: fetch fresh pool state (no TTL — arb-guard is the on-chain gate).
        let pool = match self.store.pools.get_any(&route.pool_address) {
            Some(p) => p,
            None => {
                return SimulationResult::Unprofitable {
                    reason: format!("Pool {} not found in cache", route.pool_address),
                };
            }
        };

        // Step 2: re-quote output at this input amount.
        let a_to_b = pool.token_a_mint == route.input_mint;
        let output = match pool.get_output_amount_with_cache(
            route.input_amount,
            a_to_b,
            self.store.pools.get_bin_arrays(&pool.address).as_deref(),
            self.store.pools.get_tick_arrays(&pool.address).as_deref(),
        ) {
            Some(out) if out > 0 => out,
            _ => {
                return SimulationResult::Unprofitable {
                    reason: "zero output from pool quote".to_string(),
                };
            }
        };

        // Step 3: price via current CEX mid (fall back to detection-time prices if absent).
        let cex = self.store.get_cex("SOLUSDC");
        let (cex_bid, cex_ask) = match cex {
            Some(s) => (s.best_bid_usd, s.best_ask_usd),
            None => (route.cex_bid_at_detection, route.cex_ask_at_detection),
        };

        // Step 4: compute USD values of input and output.
        let (input_usd, output_usd) = match route.direction {
            ArbDirection::BuyOnDex => {
                // We spend USDC atoms, receive SOL lamports.
                let input_usd = atoms_to_usdc(route.input_amount);
                let output_usd = lamports_to_sol(output) * cex_bid;
                (input_usd, output_usd)
            }
            ArbDirection::SellOnDex => {
                // We spend SOL lamports, receive USDC atoms.
                let input_usd = lamports_to_sol(route.input_amount) * cex_ask;
                let output_usd = atoms_to_usdc(output);
                (input_usd, output_usd)
            }
        };

        let gross_profit_usd = output_usd - input_usd;
        if gross_profit_usd <= 0.0 {
            return SimulationResult::Unprofitable {
                reason: format!("not profitable: gross_profit={:.6} usd", gross_profit_usd),
            };
        }

        // Step 5: apply slippage discount.
        let adj_profit_usd = gross_profit_usd * (1.0 - self.config.slippage_tolerance);

        // Step 6: compute worst-case tip using the highest per-relay fraction.
        let sol_price = (cex_bid + cex_ask) / 2.0;
        if sol_price <= 0.0 {
            return SimulationResult::Unprofitable {
                reason: "invalid CEX price (zero or negative)".to_string(),
            };
        }
        let adj_profit_sol = adj_profit_usd / sol_price;
        let worst_case_tip_sol = adj_profit_sol * self.config.max_tip_fraction;
        let worst_case_tip_usd = worst_case_tip_sol * sol_price;
        let tx_fee_usd = lamports_to_sol(self.config.tx_fee_lamports) * sol_price;
        let net_profit_usd_worst_case = adj_profit_usd - worst_case_tip_usd - tx_fee_usd;

        // Hard floor: net profit MUST be strictly positive, regardless of config.
        // Protects against misconfig (e.g. min_profit_usd set to 0) ever approving
        // a break-even or losing trade. Prefer to fail than send a losing tx.
        if net_profit_usd_worst_case <= 0.0 {
            return SimulationResult::Unprofitable {
                reason: format!(
                    "non-positive worst-case net: {:.6} usd (gross={:.6}, worst_tip={:.6}, fee={:.6})",
                    net_profit_usd_worst_case, gross_profit_usd, worst_case_tip_usd, tx_fee_usd,
                ),
            };
        }
        if net_profit_usd_worst_case < self.config.min_profit_usd {
            return SimulationResult::Unprofitable {
                reason: format!(
                    "below threshold: worst-case net {:.6} usd < min={:.4}",
                    net_profit_usd_worst_case, self.config.min_profit_usd,
                ),
            };
        }

        // Step 8: compute on-chain min_final_output (conservative, slippage-tolerant).
        //
        // BuyOnDex: we pay USDC, receive SOL → min SOL = (USDC_in / cex_ask) * (1 - slippage)
        // SellOnDex: we pay SOL, receive USDC → min USDC = (SOL_in * cex_bid) * (1 - slippage)
        let min_final_output = match route.direction {
            ArbDirection::BuyOnDex => {
                let min_sol = atoms_to_usdc(route.input_amount) / cex_ask;
                sol_to_lamports(min_sol * (1.0 - self.config.slippage_tolerance))
            }
            ArbDirection::SellOnDex => {
                let min_usdc = lamports_to_sol(route.input_amount) * cex_bid;
                usdc_to_atoms(min_usdc * (1.0 - self.config.slippage_tolerance))
            }
        };

        // Write back fresh values so the bundle builder gets the latest quote.
        let mut fresh_route = route.clone();
        fresh_route.expected_output = output;
        fresh_route.expected_profit_usd = adj_profit_usd;
        fresh_route.cex_bid_at_detection = cex_bid;
        fresh_route.cex_ask_at_detection = cex_ask;

        debug!(
            pool=%route.pool_address,
            direction=route.direction.label(),
            gross_usd=gross_profit_usd,
            adj_usd=adj_profit_usd,
            adj_sol=adj_profit_sol,
            worst_case_net_usd=net_profit_usd_worst_case,
            min_final_output,
            "CexDex profitable"
        );

        SimulationResult::Profitable {
            route: fresh_route,
            adjusted_profit_sol: adj_profit_sol,
            adjusted_profit_usd: adj_profit_usd,
            net_profit_usd_worst_case,
            min_final_output,
        }
    }
}
