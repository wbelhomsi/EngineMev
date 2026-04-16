//! Meteora DAMM v2 quoting math.
//!
//! DAMM v2 flat pools use constant-product math (delegates to cpmm).
//! DAMM v2 concentrated pools use CLMM single-tick math as a fallback
//! (handled by the dispatcher in pool.rs via the CLMM sqrt_price/liquidity path).

use crate::router::pool::PoolState;
use super::clmm_single_tick_output;

/// Quote a DAMM v2 swap.
///
/// If the pool has CLMM state (sqrt_price + liquidity), uses single-tick CLMM math.
/// Otherwise falls back to constant-product.
pub fn quote(pool: &PoolState, input_amount: u64, a_to_b: bool) -> Option<u64> {
    // Try CLMM single-tick math first (concentrated DAMM v2)
    if let (Some(sqrt_price_x64), Some(liquidity)) = (pool.sqrt_price_x64, pool.liquidity) {
        if sqrt_price_x64 > 0 && liquidity > 0 {
            return clmm_single_tick_output(
                pool.fee_bps,
                sqrt_price_x64,
                liquidity,
                input_amount,
                a_to_b,
            );
        }
    }

    // Fall back to constant-product
    super::cpmm::quote(pool, input_amount, a_to_b)
}
