//! Meteora DLMM bin-by-bin swap simulation.
//!
//! Walks bins starting from active_id, consuming liquidity at each bin's price.
//! Uses `ceil_div` from the parent module for fee calculations.

use crate::router::pool::{DlmmBinArray, DLMM_MAX_BIN_PER_ARRAY, PoolState};
use super::ceil_div;

/// Quote a Meteora DLMM swap using bin-by-bin simulation.
///
/// When `bin_arrays` is provided and non-empty, walks bins from the active bin.
/// Returns None if bin data is unavailable or empty (falls back to CPMM).
pub fn quote(
    pool: &PoolState,
    input_amount: u64,
    a_to_b: bool,
    bin_arrays: Option<&[DlmmBinArray]>,
) -> Option<u64> {
    let active_id = pool.current_tick?;
    let bins = bin_arrays?;
    if bins.is_empty() {
        return None;
    }
    bin_output(pool, input_amount, a_to_b, active_id, bins)
}

/// Direct bin-by-bin output calculation with explicit active_id.
/// Unlike `quote`, this does NOT return None for empty arrays --
/// it preserves the original behavior where an empty loop returns Some(0).
/// Used by `PoolState::get_dlmm_bin_output` which is a public test API.
pub fn quote_with_active_id(
    pool: &PoolState,
    input_amount: u64,
    a_to_b: bool,
    active_id: i32,
    bin_arrays: &[DlmmBinArray],
) -> Option<u64> {
    bin_output(pool, input_amount, a_to_b, active_id, bin_arrays)
}

/// DLMM bin-by-bin swap simulation.
/// Walks bins starting from active_id, consuming liquidity at each bin's price.
///
/// Per-bin swap formulas (from DEX reference):
///   X->Y: out = (in_after_fee * price) >> 64;  max_in_after_fee = (amountY << 64) / price
///   Y->X: out = (in_after_fee << 64) / price;  max_in_after_fee = (amountX * price) >> 64
///
/// Fee is applied as fee-on-amount: fee = ceil(amount * totalFee / (10^9 - totalFee))
/// For simplicity we use base fee only: baseFee = baseFactor * binStep * 10
/// The pool's fee_bps is used as an approximation (converted to the 10^9 scale).
fn bin_output(
    pool: &PoolState,
    input_amount: u64,
    a_to_b: bool,
    active_id: i32,
    bin_arrays: &[DlmmBinArray],
) -> Option<u64> {
    if input_amount == 0 {
        return Some(0);
    }

    let swap_for_y = a_to_b; // X->Y when a_to_b
    let mut amount_left = input_amount as u128;
    let mut total_out: u128 = 0;
    let mut current_id = active_id;
    let q64: u128 = 1u128 << 64;

    // Convert fee_bps to the 10^9 scale used by DLMM.
    // fee_bps=1 means 0.01% = 100_000 in 10^9 scale.
    // totalFee in 10^9 scale: fee_bps * 100_000.
    // fee_on_amount = ceil(amount * totalFee / (10^9 - totalFee))
    let total_fee_rate = (pool.fee_bps as u128) * 100_000;
    let fee_denom = 1_000_000_000u128.saturating_sub(total_fee_rate);
    if fee_denom == 0 {
        return None;
    }

    // Walk bins, max 200 as safety limit
    for _ in 0..200 {
        if amount_left == 0 {
            break;
        }

        // Find the bin in our cached arrays
        let array_idx = if current_id >= 0 {
            current_id as i64 / DLMM_MAX_BIN_PER_ARRAY as i64
        } else {
            (current_id as i64 - (DLMM_MAX_BIN_PER_ARRAY as i64 - 1))
                / DLMM_MAX_BIN_PER_ARRAY as i64
        };
        let bin_offset =
            (current_id as i64 - array_idx * DLMM_MAX_BIN_PER_ARRAY as i64) as usize;

        let bin = bin_arrays
            .iter()
            .find(|a| a.index == array_idx)
            .and_then(|a| a.bins.get(bin_offset));

        let bin = match bin {
            Some(b) => b,
            None => break, // No more bin data available
        };

        if bin.price_q64 == 0 {
            break;
        }

        if swap_for_y {
            // X->Y: need Y liquidity in this bin
            if bin.amount_y == 0 {
                current_id -= 1;
                continue;
            }

            // Max input (after fee) that this bin can absorb:
            // max_in_after_fee = ceil((amountY << 64) / price)
            let max_in_after_fee = ((bin.amount_y as u128) << 64)
                .checked_add(bin.price_q64 - 1)?
                .checked_div(bin.price_q64)?;

            // Compute fee on amount_left to get amount_after_fee
            // fee = ceil(amount_left * total_fee_rate / fee_denom)
            // Simplification: amount_after_fee = amount_left - fee
            //                = amount_left - ceil(amount_left * total_fee_rate / fee_denom)
            // Equivalently: amount_after_fee = floor(amount_left * fee_denom / (fee_denom + total_fee_rate))
            // But the DLMM protocol computes fee-on-amount as:
            //   feeAmount = ceil(amountIn * totalFee / (10^9 - totalFee))
            //   amountInAfterFee = amountIn - feeAmount
            let fee_amount = ceil_div(amount_left * total_fee_rate, fee_denom);
            let amount_after_fee = amount_left.saturating_sub(fee_amount);

            if amount_after_fee >= max_in_after_fee {
                // Consume entire bin
                total_out += bin.amount_y as u128;
                // Gross input consumed = max_in_after_fee + fee on that amount
                // fee = ceil(max_in_after_fee * total_fee_rate / fee_denom)
                let consumed_fee = ceil_div(max_in_after_fee * total_fee_rate, fee_denom);
                let consumed = max_in_after_fee + consumed_fee;
                amount_left = amount_left.saturating_sub(consumed);
                current_id -= 1;
            } else {
                // Partial fill: out = (amount_after_fee * price) >> 64
                let out = amount_after_fee
                    .checked_mul(bin.price_q64)?
                    .checked_div(q64)?;
                total_out += out;
                amount_left = 0;
            }
        } else {
            // Y->X: need X liquidity in this bin
            if bin.amount_x == 0 {
                current_id += 1;
                continue;
            }

            // Max input (after fee) that this bin can absorb:
            // max_in_after_fee = ceil((amountX * price) >> 64)
            // = ceil(amountX * price / 2^64)
            let max_in_after_fee = ceil_div(
                (bin.amount_x as u128).checked_mul(bin.price_q64)?,
                q64,
            );

            let fee_amount = ceil_div(amount_left * total_fee_rate, fee_denom);
            let amount_after_fee = amount_left.saturating_sub(fee_amount);

            if amount_after_fee >= max_in_after_fee {
                // Consume entire bin
                total_out += bin.amount_x as u128;
                let consumed_fee = ceil_div(max_in_after_fee * total_fee_rate, fee_denom);
                let consumed = max_in_after_fee + consumed_fee;
                amount_left = amount_left.saturating_sub(consumed);
                current_id += 1;
            } else {
                // Partial fill: out = (amount_after_fee << 64) / price
                let out = (amount_after_fee << 64).checked_div(bin.price_q64)?;
                total_out += out;
                amount_left = 0;
            }
        }
    }

    if total_out > u64::MAX as u128 {
        return None;
    }
    Some(total_out as u64)
}
