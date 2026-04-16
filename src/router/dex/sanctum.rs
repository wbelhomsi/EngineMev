//! Sanctum Infinity LST rate-based quoting.
//!
//! Sanctum pools use LST rate math when available, otherwise constant-product.
//! The actual LST rate conversion is handled at a higher level (sanctum.rs);
//! for basic pool quoting we use the same CLMM or CPMM paths.

use crate::router::pool::PoolState;
use super::clmm_single_tick_output;

/// Quote a Sanctum swap.
///
/// Tries CLMM single-tick math if sqrt_price + liquidity are available,
/// then falls back to constant-product.
pub fn quote(pool: &PoolState, input_amount: u64, a_to_b: bool) -> Option<u64> {
    // Try CLMM single-tick math first
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
