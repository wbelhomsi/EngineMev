//! Wraps the existing BundleBuilder to build single-leg CEX-DEX swap
//! instructions. Constructs a synthetic 1-hop ArbRoute because BundleBuilder
//! expects that shape.

use anyhow::Result;
use solana_sdk::instruction::Instruction;

use crate::cexdex::route::CexDexRoute;
use crate::executor::BundleBuilder;
use crate::router::pool::{ArbRoute, RouteHop};

/// Adapter that builds instructions for a CexDexRoute using the existing
/// multi-hop BundleBuilder. We construct a synthetic 1-hop ArbRoute where
/// base_mint is the route's input_mint.
pub fn build_instructions_for_cex_dex(
    builder: &BundleBuilder,
    route: &CexDexRoute,
    min_final_output: u64,
) -> Result<Vec<Instruction>> {
    let hop = RouteHop {
        pool_address: route.pool_address,
        dex_type: route.dex_type,
        input_mint: route.input_mint,
        output_mint: route.output_mint,
        estimated_output: route.expected_output,
    };

    // Note: ArbRoute.base_mint = input_mint (breaks the "circular" assumption
    // but BundleBuilder only uses base_mint for wSOL wrap logic, which we
    // handle explicitly via input_mint being USDC or WSOL).
    let synthetic_route = ArbRoute {
        hops: vec![hop],
        base_mint: route.input_mint,
        input_amount: route.input_amount,
        estimated_profit: 0,
        estimated_profit_lamports: 0,
    };

    builder.build_arb_instructions(&synthetic_route, min_final_output)
}
