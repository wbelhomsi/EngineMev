//! Phoenix V1 orderbook quoting math.
//!
//! Uses raw u128 best bid/ask prices from top-of-book.
//! a_to_b = selling base into bids: output_quote = input_base * best_bid
//! b_to_a = buying base from asks:  output_base = input_quote / best_ask

use crate::router::pool::PoolState;

/// Quote a Phoenix swap using top-of-book price.
///
/// Depth semantics:
///   a_to_b: token_a_reserve is the available base depth; cap input_base by it.
///   b_to_a: token_b_reserve is the available base-output depth; cap output_base by it.
pub fn quote(pool: &PoolState, input_amount: u64, a_to_b: bool) -> Option<u64> {
    // Apply fee
    let input_after_fee = (input_amount as u128)
        .checked_mul(10_000u128.checked_sub(pool.fee_bps as u128)?)?
        .checked_div(10_000)?;

    if a_to_b {
        let price = pool.best_bid_price?;
        if price == 0 {
            return None;
        }
        // Cap input (base atoms) by available bid depth (token_a_reserve)
        let effective_input = std::cmp::min(input_after_fee, pool.token_a_reserve as u128);
        let output = effective_input.checked_mul(price)?;
        if output > u64::MAX as u128 {
            return None;
        }
        Some(output as u64)
    } else {
        let price = pool.best_ask_price?;
        if price == 0 {
            return None;
        }
        // Compute uncapped output (base atoms)
        let output = input_after_fee.checked_div(price)?;
        // Cap output (base atoms) by available ask depth (token_b_reserve)
        let capped_output = std::cmp::min(output, pool.token_b_reserve as u128);
        if capped_output > u64::MAX as u128 {
            return None;
        }
        Some(capped_output as u64)
    }
}
