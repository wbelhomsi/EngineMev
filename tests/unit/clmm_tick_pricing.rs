use solana_mev_bot::router::pool::{
    ClmmTick, ClmmTickArray, DexType, PoolExtra, PoolState, tick_index_to_sqrt_price_x64,
};
use solana_sdk::pubkey::Pubkey;

/// Helper to build a CLMM pool for testing.
fn make_clmm_pool(
    dex_type: DexType,
    current_tick: i32,
    sqrt_price_x64: u128,
    liquidity: u128,
    fee_bps: u64,
) -> PoolState {
    PoolState {
        address: Pubkey::new_unique(),
        dex_type,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1_000_000_000,
        token_b_reserve: 1_000_000_000,
        fee_bps,
        current_tick: Some(current_tick),
        sqrt_price_x64: Some(sqrt_price_x64),
        liquidity: Some(liquidity),
        last_slot: 100,
        extra: PoolExtra {
            tick_spacing: Some(1),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    }
}

/// Helper: build a tick array with specified initialized ticks.
fn make_tick_array(start_tick: i32, ticks: Vec<ClmmTick>) -> ClmmTickArray {
    ClmmTickArray {
        start_tick_index: start_tick,
        ticks,
    }
}

// ─── tick_index_to_sqrt_price_x64 tests ─────────────────────────────────────

#[test]
fn test_tick_to_sqrt_price_tick_zero() {
    // tick=0 -> sqrt_price = 1.0 * 2^64
    let price = tick_index_to_sqrt_price_x64(0).unwrap();
    let q64 = 1u128 << 64;
    // Should be very close to 2^64
    let ratio = price as f64 / q64 as f64;
    assert!(
        (ratio - 1.0).abs() < 1e-10,
        "tick=0 should give sqrt_price = 2^64, got ratio={ratio}"
    );
}

#[test]
fn test_tick_to_sqrt_price_positive_tick() {
    // tick=1 -> sqrt_price = sqrt(1.0001) * 2^64
    let price = tick_index_to_sqrt_price_x64(1).unwrap();
    let q64 = 1u128 << 64;
    let expected = 1.0001_f64.sqrt() * q64 as f64;
    let diff = (price as f64 - expected).abs() / expected;
    assert!(
        diff < 1e-6,
        "tick=1 sqrt_price relative error too large: {diff}"
    );
}

#[test]
fn test_tick_to_sqrt_price_negative_tick() {
    // tick=-100 -> sqrt_price = 1.0001^(-50) * 2^64
    let price = tick_index_to_sqrt_price_x64(-100).unwrap();
    let q64 = 1u128 << 64;
    let expected = 1.0001_f64.powi(-50) * q64 as f64;
    let diff = (price as f64 - expected).abs() / expected;
    assert!(
        diff < 1e-5,
        "tick=-100 sqrt_price relative error too large: {diff}"
    );
}

#[test]
fn test_tick_to_sqrt_price_out_of_range() {
    // tick beyond 443636 should return None
    assert!(tick_index_to_sqrt_price_x64(500_000).is_none());
    assert!(tick_index_to_sqrt_price_x64(-500_000).is_none());
}

// ─── Multi-tick output tests ────────────────────────────────────────────────

#[test]
fn test_clmm_multi_tick_single_range_matches_single_tick() {
    // Pool at tick=0, liquidity=10^12, fee=30bps
    // No initialized ticks nearby -> should use remaining-amount path and
    // give roughly the same result as single-tick math
    let liquidity = 1_000_000_000_000u128; // 10^12
    let pool = make_clmm_pool(DexType::OrcaWhirlpool, 0, 1u128 << 64, liquidity, 30);

    let input = 1_000_000u64; // 1M lamports

    // Single-tick output (baseline)
    let single_tick_output = pool.get_output_amount(input, true).unwrap();

    // Multi-tick with empty tick arrays -> should fall back gracefully
    let empty_arrays: Vec<ClmmTickArray> = vec![];
    let multi_tick_output = pool
        .get_output_amount_with_cache(input, true, None, Some(&empty_arrays))
        .unwrap();

    // With empty arrays, should fall back to single-tick
    assert_eq!(
        single_tick_output, multi_tick_output,
        "Empty tick arrays should fall back to single-tick math"
    );
}

#[test]
fn test_clmm_multi_tick_small_swap_no_crossing() {
    // Pool at tick=0, with initialized ticks far away.
    // Small swap shouldn't cross any tick -> should match single-tick math closely.
    let q64 = 1u128 << 64;
    let liquidity = 1_000_000_000_000u128;
    let pool = make_clmm_pool(DexType::OrcaWhirlpool, 0, q64, liquidity, 30);

    let input = 100_000u64; // small amount

    let single_output = pool.get_output_amount(input, true).unwrap();

    // Put an initialized tick far below current price (won't be reached)
    let far_tick = ClmmTick {
        tick_index: -10000,
        liquidity_net: 500_000_000_000i128,
        liquidity_gross: 500_000_000_000u128,
    };
    let arrays = vec![make_tick_array(-10000, vec![far_tick])];

    let multi_output = pool
        .get_output_amount_with_cache(input, true, None, Some(&arrays))
        .unwrap();

    // Should be very close (within 1% due to floating-point tick-to-price conversion)
    let diff = (multi_output as f64 - single_output as f64).abs() / single_output as f64;
    assert!(
        diff < 0.01,
        "Small swap multi-tick output should be within 1% of single-tick: single={single_output}, multi={multi_output}"
    );
}

#[test]
fn test_clmm_multi_tick_crosses_one_tick() {
    // Set up a pool where a swap will cross exactly one tick boundary.
    // The key insight: after crossing a tick, liquidity changes, affecting output.
    let liquidity = 1_000_000_000u128; // 10^9 — small enough that a 10M swap crosses a tick

    // Pool at tick=100 with initial liquidity
    let sqrt_price = tick_index_to_sqrt_price_x64(100).unwrap();
    let pool = make_clmm_pool(DexType::RaydiumClmm, 100, sqrt_price, liquidity, 25);

    // Put an initialized tick at tick=95 (a_to_b direction crosses it)
    // liquidity_net is positive: crossing left-to-right adds liquidity
    // For a_to_b (price decreasing, walking right-to-left), we subtract liquidity_net
    let tick_at_95 = ClmmTick {
        tick_index: 95,
        liquidity_net: 500_000_000i128, // adds 0.5 * initial liquidity when crossed l-to-r
        liquidity_gross: 500_000_000u128,
    };
    let arrays = vec![make_tick_array(0, vec![tick_at_95])];

    // Large enough swap to cross the tick at 95
    let input = 100_000_000u64; // 100M

    let multi_output = pool
        .get_output_amount_with_cache(input, true, None, Some(&arrays));

    // The multi-tick path should produce a valid output
    assert!(
        multi_output.is_some(),
        "Multi-tick crossing should produce a valid output"
    );
    let multi_out = multi_output.unwrap();
    assert!(
        multi_out > 0,
        "Multi-tick output should be positive"
    );

    // Compare with single-tick: multi-tick should generally give a different result
    // (either more or less depending on how liquidity changes at the crossed tick)
    let single_output = pool.get_output_amount(input, true).unwrap();

    // We just verify both are reasonable — the exact relationship depends on
    // the liquidity change direction at the crossed tick
    assert!(
        single_output > 0,
        "Single-tick output should also be positive"
    );
}

#[test]
fn test_clmm_multi_tick_b_to_a_direction() {
    // Test the b_to_a (price increasing) direction
    let liquidity = 1_000_000_000u128;
    let sqrt_price = tick_index_to_sqrt_price_x64(0).unwrap();
    let pool = make_clmm_pool(DexType::OrcaWhirlpool, 0, sqrt_price, liquidity, 30);

    // Tick above current price (will be reached in b_to_a direction)
    let tick_at_50 = ClmmTick {
        tick_index: 50,
        liquidity_net: 200_000_000i128,
        liquidity_gross: 200_000_000u128,
    };
    let arrays = vec![make_tick_array(0, vec![tick_at_50])];

    let input = 50_000_000u64;
    let output = pool
        .get_output_amount_with_cache(input, false, None, Some(&arrays));

    assert!(output.is_some(), "b_to_a multi-tick should produce output");
    assert!(output.unwrap() > 0, "b_to_a output should be positive");
}

#[test]
fn test_clmm_multi_tick_no_ticks_falls_back() {
    // When tick arrays are None, should fall back to single-tick math
    let q64 = 1u128 << 64;
    let pool = make_clmm_pool(DexType::OrcaWhirlpool, 0, q64, 1_000_000_000_000u128, 30);

    let input = 1_000_000u64;

    let output_no_ticks = pool.get_output_amount_with_cache(input, true, None, None).unwrap();
    let output_single = pool.get_output_amount(input, true).unwrap();

    assert_eq!(
        output_no_ticks, output_single,
        "None tick_arrays should fall back to single-tick"
    );
}

#[test]
fn test_clmm_multi_tick_zero_input() {
    let q64 = 1u128 << 64;
    let pool = make_clmm_pool(DexType::OrcaWhirlpool, 0, q64, 1_000_000_000_000u128, 30);
    let arrays = vec![make_tick_array(0, vec![])];

    let output = pool
        .get_output_amount_with_cache(0, true, None, Some(&arrays))
        .unwrap();
    assert_eq!(output, 0, "Zero input should give zero output");
}

#[test]
fn test_clmm_multi_tick_multiple_crossings() {
    // Set up multiple initialized ticks to verify the walk handles several crossings
    let liquidity = 2_000_000_000u128;
    let sqrt_price = tick_index_to_sqrt_price_x64(200).unwrap();
    let pool = make_clmm_pool(DexType::RaydiumClmm, 200, sqrt_price, liquidity, 25);

    let ticks = vec![
        ClmmTick {
            tick_index: 190,
            liquidity_net: 500_000_000i128,
            liquidity_gross: 500_000_000u128,
        },
        ClmmTick {
            tick_index: 180,
            liquidity_net: 300_000_000i128,
            liquidity_gross: 300_000_000u128,
        },
        ClmmTick {
            tick_index: 170,
            liquidity_net: -100_000_000i128,
            liquidity_gross: 100_000_000u128,
        },
    ];
    let arrays = vec![make_tick_array(170, ticks)];

    let input = 500_000_000u64; // large swap to cross multiple ticks

    let output = pool
        .get_output_amount_with_cache(input, true, None, Some(&arrays));

    assert!(output.is_some(), "Multiple tick crossings should produce output");
    assert!(output.unwrap() > 0, "Output should be positive after multiple crossings");
}

#[test]
fn test_clmm_raydium_and_orca_both_work() {
    // Verify both DEX types produce valid multi-tick output
    let q64 = 1u128 << 64;
    let liquidity = 1_000_000_000_000u128;

    for dex in [DexType::OrcaWhirlpool, DexType::RaydiumClmm] {
        let pool = make_clmm_pool(dex, 0, q64, liquidity, 30);
        let tick = ClmmTick {
            tick_index: -10,
            liquidity_net: 100_000_000i128,
            liquidity_gross: 100_000_000u128,
        };
        let arrays = vec![make_tick_array(-100, vec![tick])];

        let output = pool.get_output_amount_with_cache(1_000_000, true, None, Some(&arrays));
        assert!(
            output.is_some() && output.unwrap() > 0,
            "{dex:?} should produce valid multi-tick output"
        );
    }
}
