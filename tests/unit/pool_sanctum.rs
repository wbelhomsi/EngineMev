use solana_sdk::pubkey::Pubkey;

use solana_mev_bot::router::pool::{DexType, PoolExtra, PoolState};

#[test]
fn test_sanctum_infinity_base_fee() {
    assert_eq!(DexType::SanctumInfinity.base_fee_bps(), 3);
}

#[test]
fn test_sanctum_virtual_pool_rate() {
    // jitoSOL rate = 1.082 SOL per jitoSOL
    // Synthetic reserves: reserve_a = 1_000_000_000_000_000, reserve_b = reserve_a / 1.082
    let rate = 1.082_f64;
    let reserve_a: u64 = 1_000_000_000_000_000;
    let reserve_b: u64 = (reserve_a as f64 / rate) as u64;

    let pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::SanctumInfinity,
        token_a_mint: Pubkey::new_unique(), // SOL
        token_b_mint: Pubkey::new_unique(), // jitoSOL
        token_a_reserve: reserve_a,
        token_b_reserve: reserve_b,
        fee_bps: 3,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
    };

    // Swap 1_000_000_000 lamports (1 jitoSOL) worth of jitoSOL -> SOL
    // With rate 1.082, expect ~1.082 SOL out minus 3bps fee
    let input = 1_000_000_000u64; // 1 jitoSOL in lamports
    let output = pool.get_output_amount(input, false).unwrap(); // b_to_a

    // Expected: ~1.082 SOL. With constant-product approximation on huge reserves,
    // price impact is negligible. Allow 0.1% tolerance.
    let expected = (input as f64 * rate * (1.0 - 3.0 / 10_000.0)) as u64;
    let diff = (output as i64 - expected as i64).unsigned_abs();
    assert!(
        diff < expected / 1000,
        "Output {} too far from expected {}, diff={}",
        output, expected, diff
    );
}

#[test]
fn test_sanctum_virtual_pool_fee_deduction() {
    // Verify that fee_bps=3 on a Sanctum pool deducts ~3bps from output
    let reserve_a: u64 = 1_000_000_000_000_000;
    let reserve_b: u64 = 1_000_000_000_000_000; // rate = 1.0 for simplicity

    let pool_with_fee = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::SanctumInfinity,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: reserve_a,
        token_b_reserve: reserve_b,
        fee_bps: 3,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
    };

    let pool_no_fee = PoolState {
        fee_bps: 0,
        ..pool_with_fee.clone()
    };

    let input = 1_000_000_000u64;
    let out_fee = pool_with_fee.get_output_amount(input, true).unwrap();
    let out_no_fee = pool_no_fee.get_output_amount(input, true).unwrap();

    // Fee pool output should be ~3bps less
    assert!(out_no_fee > out_fee);
    let fee_bps_actual = ((out_no_fee - out_fee) as f64 / out_no_fee as f64) * 10_000.0;
    assert!(
        (fee_bps_actual - 3.0).abs() < 0.5,
        "Effective fee {}bps too far from expected 3bps",
        fee_bps_actual
    );
}
