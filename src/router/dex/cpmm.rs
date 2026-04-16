//! Constant-product AMM quoting math.
//!
//! Used by RaydiumAmm, RaydiumCp, PumpSwap, and as fallback for DammV2.
//! Formula: output = (R_out * input_after_fee) / (R_in * 10000 + input_after_fee)

use crate::router::pool::PoolState;

/// Constant-product AMM output: output = (R_out * input) / (R_in + input)
pub fn quote(pool: &PoolState, input_amount: u64, a_to_b: bool) -> Option<u64> {
    let (reserve_in, reserve_out) = if a_to_b {
        (pool.token_a_reserve, pool.token_b_reserve)
    } else {
        (pool.token_b_reserve, pool.token_a_reserve)
    };

    if reserve_in == 0 || reserve_out == 0 {
        return None;
    }

    // Apply fee: input_after_fee = input * (10000 - fee_bps) / 10000
    let input_after_fee = (input_amount as u128)
        .checked_mul(10_000u128.checked_sub(pool.fee_bps as u128)?)?;

    // Constant product: output = (reserve_out * input_after_fee) / (reserve_in * 10000 + input_after_fee)
    let numerator = (reserve_out as u128).checked_mul(input_after_fee)?;
    let denominator = (reserve_in as u128)
        .checked_mul(10_000)?
        .checked_add(input_after_fee)?;

    let output = numerator.checked_div(denominator)?;

    // Sanity: output can't exceed reserves
    if output >= reserve_out as u128 {
        return None;
    }

    Some(output as u64)
}
