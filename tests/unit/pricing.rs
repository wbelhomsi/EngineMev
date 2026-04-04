use solana_mev_bot::router::pool::{DexType, PoolExtra, PoolState};
use solana_sdk::pubkey::Pubkey;

/// Helper to build a constant-product pool for testing.
fn make_cpmm_pool(reserve_a: u64, reserve_b: u64, fee_bps: u64) -> PoolState {
    PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::RaydiumAmm,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: reserve_a,
        token_b_reserve: reserve_b,
        fee_bps,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    }
}

/// Helper to build a CLMM pool for testing.
fn make_clmm_pool(
    sqrt_price_x64: u128,
    liquidity: u128,
    fee_bps: u64,
) -> PoolState {
    PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 0,
        token_b_reserve: 0,
        fee_bps,
        current_tick: Some(0),
        sqrt_price_x64: Some(sqrt_price_x64),
        liquidity: Some(liquidity),
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    }
}

// ─── CPMM Tests (tested through get_output_amount) ───────────────────────────

#[test]
fn cpmm_basic_swap_25bps() {
    // Reserves: 1_000_000 token A, 2_000_000 token B, 25 bps fee
    // Input: 10_000 token A (a_to_b)
    // amount_in_after_fee = 10_000 * (10000 - 25) / 10000 = 10_000 * 9975 = 99_750_000
    // numerator = 2_000_000 * 99_750_000 = 199_500_000_000_000
    // denominator = 1_000_000 * 10_000 + 99_750_000 = 10_000_000_000 + 99_750_000 = 10_099_750_000
    // output = 199_500_000_000_000 / 10_099_750_000 = 19_752 (truncated)
    let pool = make_cpmm_pool(1_000_000, 2_000_000, 25);
    let output = pool.get_output_amount(10_000, true).unwrap();
    assert_eq!(output, 19_752);
}

#[test]
fn cpmm_basic_swap_b_to_a() {
    // Same pool, swap B -> A
    // Reserves: in=2_000_000 (B), out=1_000_000 (A), 25 bps fee
    // Input: 10_000 token B
    // amount_in_after_fee = 10_000 * 9975 = 99_750_000
    // numerator = 1_000_000 * 99_750_000 = 99_750_000_000_000
    // denominator = 2_000_000 * 10_000 + 99_750_000 = 20_000_000_000 + 99_750_000 = 20_099_750_000
    // output = 99_750_000_000_000 / 20_099_750_000 = 4_962 (truncated)
    let pool = make_cpmm_pool(1_000_000, 2_000_000, 25);
    let output = pool.get_output_amount(10_000, false).unwrap();
    assert_eq!(output, 4_962);
}

#[test]
fn cpmm_zero_input_returns_zero() {
    let pool = make_cpmm_pool(1_000_000, 2_000_000, 25);
    let output = pool.get_output_amount(0, true).unwrap();
    assert_eq!(output, 0);
}

#[test]
fn cpmm_zero_reserve_in_returns_none() {
    let pool = make_cpmm_pool(0, 2_000_000, 25);
    let output = pool.get_output_amount(10_000, true);
    assert!(output.is_none(), "zero reserve_in should return None");
}

#[test]
fn cpmm_zero_reserve_out_returns_none() {
    let pool = make_cpmm_pool(1_000_000, 0, 25);
    let output = pool.get_output_amount(10_000, true);
    assert!(output.is_none(), "zero reserve_out should return None");
}

#[test]
fn cpmm_large_input_sublinear() {
    // Input = 10% of reserve_in. Output must be < 10% of reserve_out (constant product curve).
    let pool = make_cpmm_pool(1_000_000, 1_000_000, 0);
    let input = 100_000; // 10% of reserve
    let output = pool.get_output_amount(input, true).unwrap();

    // Proportional would be 100_000. Constant product gives less.
    assert!(
        output < 100_000,
        "constant product should give sublinear output, got {}",
        output
    );
    // But it should still be reasonably close (around ~90909 for x*y=k)
    // output = 1_000_000 * 100_000 * 10_000 / (1_000_000 * 10_000 + 100_000 * 10_000)
    //        = 1_000_000_000_000_000 / 11_000_000_000 = 90_909
    assert_eq!(output, 90_909);
}

#[test]
fn cpmm_zero_fee() {
    // Fee = 0 bps, reserves 1M/1M, input 10K
    // amount_in_after_fee = 10_000 * 10_000 = 100_000_000
    // numerator = 1_000_000 * 100_000_000 = 100_000_000_000_000
    // denominator = 1_000_000 * 10_000 + 100_000_000 = 10_100_000_000
    // output = 100_000_000_000_000 / 10_100_000_000 = 9_900 (truncated)
    let pool = make_cpmm_pool(1_000_000, 1_000_000, 0);
    let output = pool.get_output_amount(10_000, true).unwrap();
    assert_eq!(output, 9_900);
}

#[test]
fn cpmm_full_reserve_swap() {
    // Input = reserve_in. For constant product x*y=k with zero fee:
    // output = reserve_out * reserve_in / (reserve_in + reserve_in) = reserve_out / 2
    // With fee=0: output = 1_000_000 * 1_000_000 * 10_000 / (1_000_000 * 10_000 + 1_000_000 * 10_000)
    //           = 10_000_000_000_000_000 / 20_000_000_000 = 500_000
    let pool = make_cpmm_pool(1_000_000, 1_000_000, 0);
    let output = pool.get_output_amount(1_000_000, true).unwrap();
    assert_eq!(output, 500_000);
}

#[test]
fn cpmm_realistic_sol_usdc_swap() {
    // Simulate SOL/USDC pool: 50_000 SOL (in lamports) / 5_000_000 USDC (in micro-units)
    // But use realistic lamport amounts:
    // 50 SOL = 50_000_000_000 lamports, 5000 USDC = 5_000_000_000 (6 decimals)
    // Swap 1 SOL = 1_000_000_000 lamports, fee 25 bps
    let pool = make_cpmm_pool(50_000_000_000, 5_000_000_000, 25);
    let output = pool.get_output_amount(1_000_000_000, true).unwrap();
    // Expected ~98 USDC worth (price impact on 2% of pool)
    // amount_in_after_fee = 1_000_000_000 * 9975 = 9_975_000_000_000
    // numerator = 5_000_000_000 * 9_975_000_000_000 = 49_875_000_000_000_000_000_000
    // denominator = 50_000_000_000 * 10_000 + 9_975_000_000_000 = 500_000_000_000_000 + 9_975_000_000_000 = 509_975_000_000_000
    // output = 49_875_000_000_000_000_000_000 / 509_975_000_000_000 = 97_798_911
    assert_eq!(output, 97_798_911);
    // ~97.8 USDC for 1 SOL at 100 USDC/SOL price with 2% pool impact — makes sense
}

#[test]
fn cpmm_output_cannot_exceed_reserve() {
    // If math somehow gave output >= reserve_out, it should return None.
    // This can happen with extremely large inputs relative to reserves.
    // Input = 10x reserve_in, fee=0:
    // output = 1000 * 10_000 * 10_000 / (1000 * 10_000 + 10_000 * 10_000) = 100_000_000 / 110_000 = 909
    // Still under reserve_out=1000, so valid.
    let pool = make_cpmm_pool(1_000, 1_000, 0);
    let output = pool.get_output_amount(10_000, true).unwrap();
    assert_eq!(output, 909);
    assert!(output < 1_000, "output must be less than reserve_out");
}

// ─── CLMM Tests (tested through get_output_amount) ──────────────────────────

#[test]
fn clmm_basic_a_to_b() {
    // sqrt_price_x64 = 1 * 2^64 (price = 1.0, meaning 1:1 ratio)
    // liquidity = 1_000_000_000 (1B units)
    // fee_bps = 30 (0.3%, CLMM fee_rate = 3000/1_000_000)
    // input = 100_000
    let q: u128 = 1u128 << 64;
    let sqrt_price = q; // price = 1.0
    let liquidity = 1_000_000_000u128;

    let pool = make_clmm_pool(sqrt_price, liquidity, 30);
    let output = pool.get_output_amount(100_000, true);
    assert!(output.is_some(), "should produce output for valid CLMM swap");
    let out = output.unwrap();
    assert!(out > 0, "output should be positive");
    // With 0.3% fee, output should be roughly input * 0.997 for small trades
    // on a price=1.0 pool (slight slippage)
    assert!(out < 100_000, "output should be less than input after fee and slippage");
    assert!(out > 90_000, "output should be close to input for small trade, got {}", out);
}

#[test]
fn clmm_basic_b_to_a() {
    let q: u128 = 1u128 << 64;
    let sqrt_price = q; // price = 1.0
    let liquidity = 1_000_000_000u128;

    let pool = make_clmm_pool(sqrt_price, liquidity, 30);
    let output_a2b = pool.get_output_amount(100_000, true).unwrap();
    let output_b2a = pool.get_output_amount(100_000, false).unwrap();

    // For a 1:1 price pool with same input, both directions should give similar output
    let diff = (output_a2b as i64 - output_b2a as i64).unsigned_abs();
    assert!(
        diff < 1000,
        "a_to_b={} and b_to_a={} should be similar for 1:1 price pool",
        output_a2b,
        output_b2a
    );
}

#[test]
fn clmm_zero_liquidity_returns_none() {
    let q: u128 = 1u128 << 64;
    let pool = make_clmm_pool(q, 0, 30);
    // get_output_amount checks liquidity > 0 before calling get_clmm_output.
    // With liquidity=0, it falls through to CPMM which returns None (reserves are 0).
    let output = pool.get_output_amount(100_000, true);
    assert!(output.is_none(), "zero liquidity should return None");
}

#[test]
fn clmm_zero_sqrt_price_returns_none() {
    let pool = make_clmm_pool(0, 1_000_000_000, 30);
    // sqrt_price=0 → skips CLMM path, falls to CPMM with zero reserves → None
    let output = pool.get_output_amount(100_000, true);
    assert!(output.is_none(), "zero sqrt_price should return None");
}

#[test]
fn clmm_zero_input_returns_zero() {
    let q: u128 = 1u128 << 64;
    let pool = make_clmm_pool(q, 1_000_000_000, 30);
    let output = pool.get_output_amount(0, true).unwrap();
    assert_eq!(output, 0);
}

#[test]
fn clmm_large_input_no_panic() {
    // Very large input — should not panic from overflow, just return None or a valid value
    let q: u128 = 1u128 << 64;
    let liquidity = 1_000_000_000u128;
    let pool = make_clmm_pool(q, liquidity, 30);
    let _output = pool.get_output_amount(u64::MAX, true);
    // We just verify no panic. Output may be None (overflow) or a value.
}

#[test]
fn clmm_high_liquidity_low_slippage() {
    // With very high liquidity, slippage should be minimal.
    // 100T liquidity, small 1000 unit trade at price=1.0
    let q: u128 = 1u128 << 64;
    let liquidity = 100_000_000_000_000u128; // 100T
    let pool = make_clmm_pool(q, liquidity, 30);
    let output = pool.get_output_amount(1_000, true).unwrap();
    // Fee = 0.3%, so output ~ 1000 * 0.997 = 997
    assert!(
        output >= 996 && output <= 998,
        "high liquidity should give ~997 output for 1000 input at 0.3% fee, got {}",
        output
    );
}

#[test]
fn clmm_fee_rate_conversion() {
    // Verify fee rate uses 1_000_000 denominator correctly.
    // fee_bps=100 (1%) → CLMM fee_rate = 100 * 100 = 10_000 / 1_000_000 = 1%
    let q: u128 = 1u128 << 64;
    let liquidity = 100_000_000_000_000u128;

    let pool_1pct = make_clmm_pool(q, liquidity, 100);
    let pool_zero = make_clmm_pool(q, liquidity, 0);

    let out_1pct = pool_1pct.get_output_amount(10_000, true).unwrap();
    let out_zero = pool_zero.get_output_amount(10_000, true).unwrap();

    // The 1% fee pool should produce ~1% less output
    let fee_impact = out_zero - out_1pct;
    let expected_fee = out_zero / 100; // ~1%
    let tolerance = expected_fee / 10; // within 10% of expected fee impact
    assert!(
        fee_impact.abs_diff(expected_fee) <= tolerance,
        "1% fee should reduce output by ~1%, fee_impact={}, expected={}",
        fee_impact,
        expected_fee
    );
}

// ─── redact_url Tests ────────────────────────────────────────────────────────

use solana_mev_bot::config::redact_url;

#[test]
fn redact_api_key_mid_url() {
    let url = "https://mainnet.helius-rpc.com/?api-key=SECRET123&other=param";
    let redacted = redact_url(url);
    assert_eq!(
        redacted,
        "https://mainnet.helius-rpc.com/?api-key=REDACTED&other=param"
    );
}

#[test]
fn redact_api_key_at_end() {
    let url = "https://mainnet.helius-rpc.com/?api-key=SECRET123";
    let redacted = redact_url(url);
    assert_eq!(
        redacted,
        "https://mainnet.helius-rpc.com/?api-key=REDACTED"
    );
}

#[test]
fn redact_token_at_end() {
    let url = "https://example.com/stream?token=MY_SECRET_TOKEN";
    let redacted = redact_url(url);
    assert_eq!(
        redacted,
        "https://example.com/stream?token=REDACTED"
    );
}

#[test]
fn redact_x_token_mid_url() {
    let url = "https://relay.example.com/?x-token=ABCDEF&more=stuff";
    let redacted = redact_url(url);
    assert_eq!(
        redacted,
        "https://relay.example.com/?x-token=REDACTED&more=stuff"
    );
}

#[test]
fn redact_api_underscore_key() {
    let url = "https://example.com/?api_key=HIDDEN_VALUE&foo=bar";
    let redacted = redact_url(url);
    assert_eq!(
        redacted,
        "https://example.com/?api_key=REDACTED&foo=bar"
    );
}

#[test]
fn redact_no_sensitive_params_unchanged() {
    let url = "https://api.mainnet-beta.solana.com";
    let redacted = redact_url(url);
    assert_eq!(redacted, url);
}

#[test]
fn redact_empty_string() {
    let redacted = redact_url("");
    assert_eq!(redacted, "");
}

#[test]
fn redact_error_message_containing_url() {
    // redact_url treats everything from `api-key=` to the next `&` or end-of-string as the value.
    // In a free-form error message, the trailing text after the key gets consumed too.
    // This is acceptable — the function is designed for URLs, not prose.
    let msg = "Connection failed: https://helius-rpc.com/?api-key=SECRET123 timed out";
    let redacted = redact_url(msg);
    assert!(!redacted.contains("SECRET123"), "api key must be redacted");
    assert!(redacted.contains("api-key=REDACTED"), "should contain redacted placeholder");
}

#[test]
fn redact_error_message_url_with_ampersand() {
    // When the URL has proper query params, redaction is precise even in error messages
    let msg = "Error: https://rpc.com/?api-key=SECRET&retry=3 failed";
    let redacted = redact_url(msg);
    assert_eq!(
        redacted,
        "Error: https://rpc.com/?api-key=REDACTED&retry=3 failed"
    );
}

#[test]
fn redact_multiple_sensitive_params() {
    // URL with both api-key and token
    let url = "https://example.com/?api-key=KEY1&token=TOK2";
    let redacted = redact_url(url);
    assert!(
        !redacted.contains("KEY1"),
        "api-key value should be redacted, got: {}",
        redacted
    );
    assert!(
        !redacted.contains("TOK2"),
        "token value should be redacted, got: {}",
        redacted
    );
}
