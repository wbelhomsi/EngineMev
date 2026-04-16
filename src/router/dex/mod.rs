//! Per-DEX quoting math extracted from `pool.rs`.
//!
//! Each sub-module exposes a `quote(pool, input_amount, a_to_b, ...)` free function
//! that calculates the output amount for a swap on that DEX type.
//! The dispatcher in `PoolState::get_output_amount_with_cache` routes to these.

pub mod clmm_orca;
pub mod clmm_raydium;
pub mod cpmm;
pub mod damm_v2;
pub mod dlmm;
pub mod manifest;
pub mod phoenix;
pub mod sanctum;

// ─── Shared CLMM / DLMM math helpers ───────────────────────────────────────

/// Ceiling division: ceil(a / b). Returns 0 if b == 0.
#[inline]
pub fn ceil_div(a: u128, b: u128) -> u128 {
    if b == 0 {
        return 0;
    }
    a.div_ceil(b)
}

/// Result of a single constant-liquidity swap step.
pub struct SwapStepResult {
    pub amount_in: u128,
    pub amount_out: u128,
    pub next_sqrt_price: u128,
}

/// Minimum sqrt_price_x64 (tick = -443636).
pub const MIN_SQRT_PRICE_X64: u128 = 4295048016;
/// Maximum sqrt_price_x64 (tick = 443636).
pub const MAX_SQRT_PRICE_X64: u128 = 79226673515401279992447579055;

/// Compute swap amounts within a single constant-liquidity range.
///
/// Formulas (P = sqrt_price Q64.64, L = liquidity, Q = 2^64):
///   a_to_b (price decreasing, token A in, token B out):
///     max_amount_in = L * Q * (current_P - target_P) / (current_P * target_P)
///     amount_out = L * (current_P - new_P) / Q
///   b_to_a (price increasing, token B in, token A out):
///     max_amount_in = L * (target_P - current_P) / Q
///     amount_out = L * Q * (new_P - P) / (P * new_P)
///
/// Returns (amount_in_consumed, amount_out, next_sqrt_price).
pub fn compute_swap_step(
    current_sqrt_price: u128,
    target_sqrt_price: u128,
    liquidity: u128,
    amount_remaining: u128,
    a_to_b: bool,
) -> Option<(u128, u128, u128)> {
    let q: u128 = 1u128 << 64;

    if a_to_b {
        // Price decreasing. Token A in, Token B out.
        let price_diff = current_sqrt_price.checked_sub(target_sqrt_price)?;
        // max_in = L * price_diff / current_P * Q / target_P
        // Reorder to avoid overflow: (L * price_diff / current_P) * (Q / target_P)
        // But that loses precision. Better: L * (Q / target_P - Q / current_P)
        // = L * Q * (current_P - target_P) / (current_P * target_P)
        // To avoid overflow, compute in steps:
        // step1 = L * price_diff / current_P  (fits u128 for reasonable L)
        // max_in = step1 * Q / target_P
        let step1 = liquidity.checked_mul(price_diff)?.checked_div(current_sqrt_price)?;
        let max_in = step1.checked_mul(q)?.checked_div(target_sqrt_price)?;

        let (amount_in, next_price) = if amount_remaining >= max_in {
            (max_in, target_sqrt_price)
        } else {
            // Partial fill: new_P = L * P / (L + amount * P / Q)
            let amt_x_price = amount_remaining.checked_mul(current_sqrt_price)?.checked_div(q)?;
            let denom = liquidity.checked_add(amt_x_price)?;
            if denom == 0 {
                return None;
            }
            let new_price = liquidity.checked_mul(current_sqrt_price)?.checked_div(denom)?;
            (amount_remaining, new_price)
        };

        // amount_out = L * (current_P - new_P) / Q
        let out_price_diff = current_sqrt_price.checked_sub(next_price)?;
        let amount_out = liquidity.checked_mul(out_price_diff)?.checked_div(q)?;

        Some((amount_in, amount_out, next_price))
    } else {
        // Price increasing. Token B in, Token A out.
        let price_diff = target_sqrt_price.checked_sub(current_sqrt_price)?;
        // max_in = L * price_diff / Q
        let max_in = liquidity.checked_mul(price_diff)?.checked_div(q)?;

        let (amount_in, next_price) = if amount_remaining >= max_in {
            (max_in, target_sqrt_price)
        } else {
            // Partial fill: new_P = P + amount * Q / L
            let delta = amount_remaining.checked_mul(q)?.checked_div(liquidity)?;
            let new_price = current_sqrt_price.checked_add(delta)?;
            (amount_remaining, new_price)
        };

        // amount_out = L * Q * (new_P - P) / (P * new_P)
        // Split: (L * (new_P - P) / P) * (Q / new_P)
        let out_price_diff = next_price.checked_sub(current_sqrt_price)?;
        let numerator = liquidity.checked_mul(out_price_diff)?;
        let step1 = numerator.checked_div(current_sqrt_price)?;
        let amount_out = step1.checked_mul(q)?.checked_div(next_price)?;

        Some((amount_in, amount_out, next_price))
    }
}

/// Convert tick index to sqrt_price in Q64.64 format.
///
/// sqrt_price = 1.0001^(tick/2) * 2^64
///
/// Uses f64 for the off-chain quoter. This is acceptable because:
/// - f64 has ~15 significant digits, sufficient for route discovery
/// - The on-chain program uses exact integer math; we only need approximate prices
///   to decide whether to submit a bundle
/// - The key constraint (pitfall #15) about u128 math applies to the swap math
///   (L*P products), not to this conversion function
pub fn tick_index_to_sqrt_price_x64(tick: i32) -> Option<u128> {
    let abs_tick = tick.unsigned_abs();
    if abs_tick > 443636 {
        return None;
    }

    // 1.0001^(tick/2) = 1.00005^tick (since sqrt(1.0001) = 1.00005 approximately)
    // More precisely: sqrt(1.0001) = 1.000049998750...
    // For f64, using powi is sufficiently accurate for route discovery.
    let sqrt_price_f64 = 1.0001_f64.powi(tick / 2)
        * if tick % 2 != 0 {
            if tick > 0 { 1.0001_f64.sqrt() } else { 1.0 / 1.0001_f64.sqrt() }
        } else {
            1.0
        };

    let q64 = (1u128 << 64) as f64;
    let result = sqrt_price_f64 * q64;

    if result <= 0.0 || result >= u128::MAX as f64 {
        return None;
    }

    Some(result as u128)
}

/// Single-tick CLMM output calculation using u128 integer math.
/// For Orca Whirlpool, Raydium CLMM, DAMM v2 concentrated.
///
/// Fee rate uses 1,000,000 denominator (not 10,000 basis points).
/// CLMM feeRate = fee_bps * 100 (e.g., 0.3% fee = fee_bps=30, feeRate=3000).
///
/// Formulas (P = sqrt_price in Q64.64, L = liquidity, Q = 2^64):
///   a_to_b: new_P = L*P / (L + input*P/Q),  output = L*(P - new_P)/Q
///   b_to_a: new_P = P + input*Q/L,  output = L*(1/P - 1/new_P)*Q
///
/// Returns None on overflow or zero output -- conservative for route discovery.
pub fn clmm_single_tick_output(
    fee_bps: u64,
    sqrt_price_x64: u128,
    liquidity: u128,
    input_amount: u64,
    a_to_b: bool,
) -> Option<u64> {
    let q: u128 = 1u128 << 64;

    // Fee: CLMM uses 1,000,000 denominator. fee_bps * 100 converts to CLMM rate.
    let fee_rate = fee_bps as u128 * 100;
    let fee_denom: u128 = 1_000_000;
    let input_after_fee = (input_amount as u128)
        .checked_mul(fee_denom.checked_sub(fee_rate)?)?
        .checked_div(fee_denom)?;

    if input_after_fee == 0 {
        return Some(0);
    }

    if a_to_b {
        // Sell token A, get token B. sqrt_price goes down.
        // new_P = L * P / (L + input * P / Q)
        // Rearranged to avoid overflow: new_P = (L * P) / (L + input * P / Q)
        let input_x_price = input_after_fee
            .checked_mul(sqrt_price_x64)?
            .checked_div(q)?;
        let denom = liquidity.checked_add(input_x_price)?;
        if denom == 0 { return None; }
        let new_sqrt_price = liquidity
            .checked_mul(sqrt_price_x64)?
            .checked_div(denom)?;

        if new_sqrt_price >= sqrt_price_x64 { return None; }

        // output = L * (P - new_P) / Q
        let price_diff = sqrt_price_x64.checked_sub(new_sqrt_price)?;
        let output = liquidity
            .checked_mul(price_diff)?
            .checked_div(q)?;

        if output > u64::MAX as u128 { return None; }
        Some(output as u64)
    } else {
        // Sell token B, get token A. sqrt_price goes up.
        // new_P = P + input * Q / L
        let price_delta = input_after_fee
            .checked_mul(q)?
            .checked_div(liquidity)?;
        let new_sqrt_price = sqrt_price_x64.checked_add(price_delta)?;

        if new_sqrt_price <= sqrt_price_x64 { return None; }

        // output = L * Q * (new_P - P) / (P * new_P)
        // To avoid overflow of L * Q (which exceeds u128 when L > 2^64),
        // rearrange: output = L * (Q / P - Q / new_P)
        //                   = L * Q * price_delta / (P * new_P)
        // Since price_delta = input * Q / L, substitute:
        //   output = input * Q^2 / (P * new_P)
        // But Q^2 = 2^128 which overflows u128.
        //
        // Safe approach: split into (L * price_delta / P) * (Q / new_P)
        // First: L * price_delta may overflow, but try it:
        let numerator = liquidity.checked_mul(price_delta)?;
        // Then: numerator * Q / (P * new_P)
        // = (numerator / P) * (Q / new_P)
        let step1 = numerator.checked_div(sqrt_price_x64)?;
        let output = step1.checked_mul(q)?.checked_div(new_sqrt_price)?;

        if output > u64::MAX as u128 { return None; }
        Some(output as u64)
    }
}
