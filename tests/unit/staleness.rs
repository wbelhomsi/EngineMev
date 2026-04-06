/// Verify that route calculator does NOT find routes when pool state is stale.
/// Pools older than TTL should not produce routes.
#[test]
fn test_stale_pool_produces_no_routes() {
    use std::time::Duration;
    use solana_sdk::pubkey::Pubkey;
    use solana_mev_bot::config;
    use solana_mev_bot::router::RouteCalculator;
    use solana_mev_bot::router::pool::{DexType, DetectedSwap, PoolExtra, PoolState};
    use solana_mev_bot::state::StateCache;

    // TTL = 1 second for this test
    let cache = StateCache::new(Duration::from_secs(1));
    let sol = config::sol_mint();
    let token = Pubkey::new_unique();

    // Add two pools
    let pool_a = Pubkey::new_unique();
    cache.upsert(pool_a, PoolState {
        address: pool_a,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: sol,
        token_b_mint: token,
        token_a_reserve: 100_000_000_000_000,
        token_b_reserve: 100_000_000_000_000,
        fee_bps: 25,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    });

    let pool_b = Pubkey::new_unique();
    cache.upsert(pool_b, PoolState {
        address: pool_b,
        dex_type: DexType::RaydiumCp,
        token_a_mint: sol,
        token_b_mint: token,
        token_a_reserve: 100_000_000_000_000,
        token_b_reserve: 110_000_000_000_000, // 10% spread
        fee_bps: 25,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    });

    let calculator = RouteCalculator::new(cache, 3);
    let trigger = DetectedSwap {
        dex_type: DexType::OrcaWhirlpool,
        pool_address: pool_a,
        input_mint: sol,
        output_mint: token,
        amount: None,
        observed_slot: 100,
    };

    // Immediately: should find routes
    let routes = calculator.find_routes(&trigger);
    assert!(!routes.is_empty(), "Fresh pools should produce routes");

    // Wait for TTL to expire
    std::thread::sleep(Duration::from_secs(2));

    // After TTL: should NOT find routes (stale data)
    let routes = calculator.find_routes(&trigger);
    assert!(routes.is_empty(), "Stale pools should NOT produce routes");
}
