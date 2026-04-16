//! Raydium CLMM multi-tick crossing quoting math.
//!
//! Same structure as Orca Whirlpool but with Raydium-specific tick spacing.
//! Falls back to `clmm_single_tick_output` when tick arrays are absent.

use crate::router::pool::{ClmmTickArray, PoolState};
use super::{
    clmm_single_tick_output, compute_swap_step, tick_index_to_sqrt_price_x64,
    MIN_SQRT_PRICE_X64, MAX_SQRT_PRICE_X64,
};

/// Quote a Raydium CLMM swap with optional multi-tick crossing.
///
/// When `tick_arrays` is provided and non-empty, uses multi-tick simulation.
/// Falls back to single-tick CLMM math otherwise.
pub fn quote(
    pool: &PoolState,
    input_amount: u64,
    a_to_b: bool,
    tick_arrays: Option<&[ClmmTickArray]>,
) -> Option<u64> {
    let (sqrt_price, liquidity) = match (pool.sqrt_price_x64, pool.liquidity) {
        (Some(sp), Some(liq)) if sp > 0 && liq > 0 => (sp, liq),
        _ => return None,
    };

    if let Some(ticks) = tick_arrays {
        if !ticks.is_empty() {
            return multi_tick_output(pool, input_amount, a_to_b, sqrt_price, liquidity, ticks);
        }
    }

    // Fallback: single-tick
    clmm_single_tick_output(pool.fee_bps, sqrt_price, liquidity, input_amount, a_to_b)
}

/// Multi-tick CLMM swap simulation for Raydium CLMM.
/// Walks initialized ticks, adjusting liquidity at each boundary.
/// More accurate than single-tick math for swaps that cross tick boundaries.
///
/// Returns None on overflow or zero output -- conservative for route discovery.
fn multi_tick_output(
    pool: &PoolState,
    input_amount: u64,
    a_to_b: bool,
    sqrt_price_x64: u128,
    liquidity: u128,
    tick_arrays: &[ClmmTickArray],
) -> Option<u64> {
    let fee_rate = pool.fee_bps as u128 * 100;
    let fee_denom: u128 = 1_000_000;

    // Apply fee upfront (matches single-tick approach)
    let mut amount_remaining = (input_amount as u128)
        .checked_mul(fee_denom.checked_sub(fee_rate)?)?
        .checked_div(fee_denom)?;

    if amount_remaining == 0 {
        return Some(0);
    }

    let mut total_output: u128 = 0;
    let mut current_sqrt_price = sqrt_price_x64;
    let mut current_liquidity = liquidity;
    let current_tick = pool.current_tick.unwrap_or(0);

    // Collect all initialized ticks from the arrays, sorted for traversal
    let mut initialized_ticks: Vec<_> = tick_arrays
        .iter()
        .flat_map(|arr| arr.ticks.iter())
        .filter(|t| t.liquidity_gross > 0)
        .collect();

    if a_to_b {
        // Price decreasing: walk ticks below current in descending order
        initialized_ticks.retain(|t| t.tick_index <= current_tick);
        initialized_ticks.sort_by(|a, b| b.tick_index.cmp(&a.tick_index));
    } else {
        // Price increasing: walk ticks above current in ascending order
        initialized_ticks.retain(|t| t.tick_index > current_tick);
        initialized_ticks.sort_by(|a, b| a.tick_index.cmp(&b.tick_index));
    }

    // Safety limit to prevent infinite loops on bad data
    let max_steps = 50;

    for (steps, tick) in initialized_ticks.iter().enumerate() {
        if amount_remaining == 0 || steps >= max_steps {
            break;
        }

        let target_sqrt_price = tick_index_to_sqrt_price_x64(tick.tick_index)?;

        // Skip ticks that are in the wrong direction relative to current price
        if a_to_b && target_sqrt_price >= current_sqrt_price {
            // Cross the tick to update liquidity even if price hasn't moved
            current_liquidity =
                (current_liquidity as i128).checked_sub(tick.liquidity_net)? as u128;
            continue;
        }
        if !a_to_b && target_sqrt_price <= current_sqrt_price {
            current_liquidity =
                (current_liquidity as i128).checked_add(tick.liquidity_net)? as u128;
            continue;
        }

        if current_liquidity == 0 {
            // No liquidity in this range, jump to the tick boundary
            current_sqrt_price = target_sqrt_price;
            if a_to_b {
                current_liquidity =
                    (current_liquidity as i128).checked_sub(tick.liquidity_net)? as u128;
            } else {
                current_liquidity =
                    (current_liquidity as i128).checked_add(tick.liquidity_net)? as u128;
            }
            continue;
        }

        // Compute swap step within this constant-liquidity range
        let (amount_in, amount_out, next_sqrt_price) = compute_swap_step(
            current_sqrt_price,
            target_sqrt_price,
            current_liquidity,
            amount_remaining,
            a_to_b,
        )?;

        amount_remaining = amount_remaining.saturating_sub(amount_in);
        total_output += amount_out;
        current_sqrt_price = next_sqrt_price;

        // If we reached the tick boundary, cross it (adjust liquidity)
        if current_sqrt_price == target_sqrt_price {
            if a_to_b {
                current_liquidity =
                    (current_liquidity as i128).checked_sub(tick.liquidity_net)? as u128;
            } else {
                current_liquidity =
                    (current_liquidity as i128).checked_add(tick.liquidity_net)? as u128;
            }
        }
    }

    // Handle remaining amount in the last range (no more initialized ticks)
    if amount_remaining > 0 && current_liquidity > 0 {
        // Use sqrt_price limits as boundary (MIN_SQRT_PRICE / MAX_SQRT_PRICE)
        let limit_price = if a_to_b {
            MIN_SQRT_PRICE_X64
        } else {
            MAX_SQRT_PRICE_X64
        };
        if let Some((_, amt_out, _)) = compute_swap_step(
            current_sqrt_price,
            limit_price,
            current_liquidity,
            amount_remaining,
            a_to_b,
        ) {
            total_output += amt_out;
        }
    }

    if total_output > u64::MAX as u128 {
        return None;
    }
    Some(total_output as u64)
}
