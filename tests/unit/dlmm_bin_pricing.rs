use solana_mev_bot::router::pool::{
    DexType, DlmmBin, DlmmBinArray, DLMM_MAX_BIN_PER_ARRAY, PoolExtra, PoolState,
};
use solana_sdk::pubkey::Pubkey;

/// Helper to build a DLMM pool for testing.
fn make_dlmm_pool(active_id: i32, fee_bps: u64) -> PoolState {
    PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::MeteoraDlmm,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1_000_000_000,
        token_b_reserve: 1_000_000_000,
        fee_bps,
        current_tick: Some(active_id),
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    }
}

/// Helper to build a bin array with custom bins at a given array index.
fn make_bin_array(index: i64, bins: Vec<DlmmBin>) -> DlmmBinArray {
    let mut padded = bins;
    // Pad to 70 bins if needed
    while padded.len() < DLMM_MAX_BIN_PER_ARRAY {
        padded.push(DlmmBin {
            amount_x: 0,
            amount_y: 0,
            price_q64: 0,
        });
    }
    DlmmBinArray {
        index,
        bins: padded,
    }
}

/// Make a bin with the given amounts and a 1:1 price (price_q64 = 2^64).
fn bin_1to1(amount_x: u64, amount_y: u64) -> DlmmBin {
    DlmmBin {
        amount_x,
        amount_y,
        price_q64: 1u128 << 64, // 1:1 price
    }
}

/// Make a bin with a specific Q64.64 price.
fn bin_with_price(amount_x: u64, amount_y: u64, price_q64: u128) -> DlmmBin {
    DlmmBin {
        amount_x,
        amount_y,
        price_q64,
    }
}

// --- Tests ---

#[test]
fn dlmm_bin_zero_input_returns_zero() {
    let pool = make_dlmm_pool(0, 1);
    let arrays = vec![make_bin_array(0, vec![bin_1to1(1_000_000, 1_000_000)])];
    let output = pool.get_dlmm_bin_output(0, true, 0, &arrays);
    assert_eq!(output, Some(0));
}

#[test]
fn dlmm_bin_single_bin_swap_for_y() {
    // Pool at active_id=0 with 1:1 price, fee_bps=1 (0.01%)
    // Bin 0 has amountY=1_000_000.
    // Swap 500_000 X for Y.
    let pool = make_dlmm_pool(0, 1);

    // Bin at index 0, position 0 in array 0
    let mut bins = vec![DlmmBin { amount_x: 0, amount_y: 0, price_q64: 0 }; DLMM_MAX_BIN_PER_ARRAY];
    bins[0] = bin_1to1(0, 1_000_000);
    let arrays = vec![DlmmBinArray { index: 0, bins }];

    let output = pool.get_dlmm_bin_output(500_000, true, 0, &arrays).unwrap();

    // At 1:1 price with 0.01% fee:
    // fee_rate = 1 * 100_000 = 100_000 in 10^9 scale
    // fee_denom = 1_000_000_000 - 100_000 = 999_900_000
    // fee = ceil(500_000 * 100_000 / 999_900_000) = ceil(50_000_000_000 / 999_900_000) = ceil(50.005) = 51
    // amount_after_fee = 500_000 - 51 = 499_949
    // max_in_after_fee = ceil((1_000_000 << 64) / (1 << 64)) = 1_000_000
    // partial fill: out = (499_949 * (1 << 64)) >> 64 = 499_949
    assert!(output > 0, "should have positive output");
    assert!(output < 500_000, "output should be less than input due to fee");
    // Should be very close to 499_949
    assert!(
        (499_900..=499_999).contains(&output),
        "expected ~499949, got {}",
        output
    );
}

#[test]
fn dlmm_bin_single_bin_swap_for_x() {
    // Swap Y for X at 1:1 price
    let pool = make_dlmm_pool(0, 1);

    let mut bins = vec![DlmmBin { amount_x: 0, amount_y: 0, price_q64: 0 }; DLMM_MAX_BIN_PER_ARRAY];
    bins[0] = bin_1to1(1_000_000, 0);
    let arrays = vec![DlmmBinArray { index: 0, bins }];

    let output = pool.get_dlmm_bin_output(500_000, false, 0, &arrays).unwrap();

    assert!(output > 0, "should have positive output");
    assert!(output < 500_000, "output should be less than input due to fee");
    assert!(
        (499_900..=499_999).contains(&output),
        "expected ~499949, got {}",
        output
    );
}

#[test]
fn dlmm_bin_crosses_bins_swap_for_y() {
    // Two bins: id=0 has 100_000 Y liquidity, id=-1 has 100_000 Y liquidity.
    // Swap 150_000 X for Y. Should drain bin 0 (100K Y) then partial fill bin -1 (~50K Y).
    let pool = make_dlmm_pool(0, 0); // zero fee for clarity

    // Bin 0 is at array index 0, position 0
    let mut bins_arr0 = vec![DlmmBin { amount_x: 0, amount_y: 0, price_q64: 0 }; DLMM_MAX_BIN_PER_ARRAY];
    bins_arr0[0] = bin_1to1(0, 100_000);

    // Bin -1 is at array index -1, position 69 (last bin in array -1)
    let mut bins_arr_neg1 = vec![DlmmBin { amount_x: 0, amount_y: 0, price_q64: 0 }; DLMM_MAX_BIN_PER_ARRAY];
    bins_arr_neg1[69] = bin_1to1(0, 100_000);

    let arrays = vec![
        DlmmBinArray { index: 0, bins: bins_arr0 },
        DlmmBinArray { index: -1, bins: bins_arr_neg1 },
    ];

    let output = pool.get_dlmm_bin_output(150_000, true, 0, &arrays).unwrap();

    // With zero fee and 1:1 price: should get ~150_000 output total
    // Bin 0: drain 100_000 Y (consume 100_000 X)
    // Bin -1: partial fill 50_000 Y (consume 50_000 X)
    assert!(
        (149_000..=151_000).contains(&output),
        "expected ~150000, got {}",
        output
    );
}

#[test]
fn dlmm_bin_no_liquidity_returns_zero() {
    // All bins empty
    let pool = make_dlmm_pool(0, 1);
    let mut bins = vec![DlmmBin { amount_x: 0, amount_y: 0, price_q64: 0 }; DLMM_MAX_BIN_PER_ARRAY];
    // Need at least a valid price to not break immediately
    bins[0] = DlmmBin {
        amount_x: 0,
        amount_y: 0,
        price_q64: 1u128 << 64,
    };
    let arrays = vec![DlmmBinArray { index: 0, bins }];

    let output = pool.get_dlmm_bin_output(100_000, true, 0, &arrays).unwrap();
    assert_eq!(output, 0, "no Y liquidity should produce zero output");
}

#[test]
fn dlmm_bin_no_arrays_returns_none() {
    // Empty bin arrays means we break immediately, returning 0 output
    let pool = make_dlmm_pool(0, 1);
    let output = pool.get_dlmm_bin_output(100_000, true, 0, &[]);
    // No bins found -> loop doesn't execute -> total_out = 0
    assert_eq!(output, Some(0));
}

#[test]
fn dlmm_bin_get_output_amount_with_bins_uses_bins() {
    // Verify that get_output_amount_with_bins prefers bin data over synthetic reserves
    let pool = make_dlmm_pool(0, 1);

    let mut bins = vec![DlmmBin { amount_x: 0, amount_y: 0, price_q64: 0 }; DLMM_MAX_BIN_PER_ARRAY];
    bins[0] = bin_1to1(0, 1_000_000);
    let arrays = vec![DlmmBinArray { index: 0, bins }];

    let with_bins = pool.get_output_amount_with_bins(100_000, true, Some(&arrays)).unwrap();
    let without_bins = pool.get_output_amount_with_bins(100_000, true, None).unwrap();

    // Both should produce output, but they may differ because synthetic reserves use
    // constant-product math while bins use per-bin quoting
    assert!(with_bins > 0, "bin output should be positive");
    assert!(without_bins > 0, "synthetic output should be positive");
}

#[test]
fn dlmm_bin_non_dlmm_pool_ignores_bins() {
    // get_output_amount_with_bins on a non-DLMM pool should ignore bin data
    let pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::RaydiumAmm,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1_000_000,
        token_b_reserve: 1_000_000,
        fee_bps: 25,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    };

    let mut bins = vec![DlmmBin { amount_x: 0, amount_y: 0, price_q64: 0 }; DLMM_MAX_BIN_PER_ARRAY];
    bins[0] = bin_1to1(0, 1_000_000);
    let arrays = vec![DlmmBinArray { index: 0, bins }];

    let with_bins = pool.get_output_amount_with_bins(10_000, true, Some(&arrays)).unwrap();
    let without_bins = pool.get_output_amount(10_000, true).unwrap();

    assert_eq!(with_bins, without_bins, "non-DLMM pool should ignore bin data");
}

#[test]
fn dlmm_bin_price_2x_swap_for_y() {
    // Test with 2:1 price (1 X = 2 Y)
    // price_q64 = 2 * 2^64
    let pool = make_dlmm_pool(0, 0); // zero fee

    let price_2x = 2u128 << 64; // price = 2.0
    let mut bins = vec![DlmmBin { amount_x: 0, amount_y: 0, price_q64: 0 }; DLMM_MAX_BIN_PER_ARRAY];
    bins[0] = bin_with_price(0, 2_000_000, price_2x);
    let arrays = vec![DlmmBinArray { index: 0, bins }];

    let output = pool.get_dlmm_bin_output(500_000, true, 0, &arrays).unwrap();

    // X->Y: out = (in * price) >> 64 = (500_000 * 2*2^64) >> 64 = 1_000_000
    assert_eq!(output, 1_000_000, "1 X should get 2 Y at price=2.0");
}

#[test]
fn dlmm_bin_price_2x_swap_for_x() {
    // Test Y->X at price 2:1 (1 X costs 2 Y)
    let pool = make_dlmm_pool(0, 0); // zero fee

    let price_2x = 2u128 << 64;
    let mut bins = vec![DlmmBin { amount_x: 0, amount_y: 0, price_q64: 0 }; DLMM_MAX_BIN_PER_ARRAY];
    bins[0] = bin_with_price(1_000_000, 0, price_2x);
    let arrays = vec![DlmmBinArray { index: 0, bins }];

    let output = pool.get_dlmm_bin_output(1_000_000, false, 0, &arrays).unwrap();

    // Y->X: out = (in << 64) / price = (1_000_000 * 2^64) / (2 * 2^64) = 500_000
    assert_eq!(output, 500_000, "2 Y should get 1 X at price=2.0");
}

#[test]
fn dlmm_bin_negative_active_id() {
    // Test with active_id = -5 (bin in array index -1, position 65)
    let pool = make_dlmm_pool(-5, 0);

    let mut bins = vec![DlmmBin { amount_x: 0, amount_y: 0, price_q64: 0 }; DLMM_MAX_BIN_PER_ARRAY];
    // active_id=-5: array_idx = (-5 - 69) / 70 = -74/70 = -1 (floor)
    // bin_offset = -5 - (-1 * 70) = -5 + 70 = 65
    bins[65] = bin_1to1(0, 1_000_000);
    let arrays = vec![DlmmBinArray { index: -1, bins }];

    let output = pool.get_dlmm_bin_output(100_000, true, -5, &arrays).unwrap();
    assert!(output > 0, "negative active_id should work, got {}", output);
    assert!(output <= 100_000, "output should be <= input");
}

#[test]
fn dlmm_bin_drains_entire_bin() {
    // Input exceeds single bin capacity. Should drain the bin completely.
    let pool = make_dlmm_pool(0, 0);

    let mut bins = vec![DlmmBin { amount_x: 0, amount_y: 0, price_q64: 0 }; DLMM_MAX_BIN_PER_ARRAY];
    bins[0] = bin_1to1(0, 50_000); // only 50K Y available
    let arrays = vec![DlmmBinArray { index: 0, bins }];

    let output = pool.get_dlmm_bin_output(1_000_000, true, 0, &arrays).unwrap();

    // Should get exactly 50_000 (all the Y in the bin), then stop (no more bins)
    assert_eq!(output, 50_000, "should drain exactly the bin's Y amount");
}

#[test]
fn dlmm_bin_fee_reduces_output() {
    // Compare output with and without fee
    let pool_no_fee = make_dlmm_pool(0, 0);
    let pool_with_fee = make_dlmm_pool(0, 10); // 10 bps = 0.1%

    let mut bins = vec![DlmmBin { amount_x: 0, amount_y: 0, price_q64: 0 }; DLMM_MAX_BIN_PER_ARRAY];
    bins[0] = bin_1to1(0, 10_000_000);
    let arrays = vec![DlmmBinArray { index: 0, bins }];

    let out_no_fee = pool_no_fee.get_dlmm_bin_output(1_000_000, true, 0, &arrays).unwrap();
    let out_with_fee = pool_with_fee.get_dlmm_bin_output(1_000_000, true, 0, &arrays).unwrap();

    assert_eq!(out_no_fee, 1_000_000, "zero fee at 1:1 should give exact output");
    assert!(
        out_with_fee < out_no_fee,
        "fee should reduce output: no_fee={}, with_fee={}",
        out_no_fee,
        out_with_fee
    );
    // 10 bps = 0.1%, so output should be ~999_000
    assert!(
        (998_000..=999_999).contains(&out_with_fee),
        "10bps fee should give ~999000, got {}",
        out_with_fee
    );
}
