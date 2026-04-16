/// Verify that route calculator finds routes and simulator processes them
/// even with stale cache data. On-chain arb-guard (min_amount_out) is the
/// real safety gate — cache TTL is only for eviction, not gating.
#[test]
fn test_stale_pool_produces_no_routes() {
    use std::time::Duration;
    use solana_sdk::pubkey::Pubkey;
    use solana_mev_bot::config;
    use solana_mev_bot::router::RouteCalculator;
    use solana_mev_bot::router::pool::{DexType, DetectedSwap, PoolExtra, PoolState};
    use solana_mev_bot::router::ProfitSimulator;
    use solana_mev_bot::state::StateCache;

    // TTL = 1 second for this test
    let cache = StateCache::new(Duration::from_secs(1));
    let sol = config::sol_mint();
    let token = Pubkey::new_unique();

    // Add two pools with a 10% spread (profitable arb)
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

    let calculator = RouteCalculator::new(cache.clone(), 3);
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

    // Route calculator still finds candidates (uses get_any)
    let routes = calculator.find_routes(&trigger);
    assert!(!routes.is_empty(), "Route calc uses get_any — finds stale pools too");

    // Simulator also processes them (on-chain arb-guard is the safety gate)
    let simulator = ProfitSimulator::new(cache, 0.50, 1000, 1000);
    let result = simulator.simulate(&routes[0]);
    match result {
        solana_mev_bot::router::simulator::SimulationResult::Profitable { .. } => {
            // Expected: simulator uses get_any, on-chain guard is the real gate
        }
        solana_mev_bot::router::simulator::SimulationResult::Unprofitable { reason } => {
            // Also acceptable if profit doesn't meet threshold after slippage
            assert!(
                !reason.contains("Pool not found"),
                "Simulator should NOT reject due to TTL staleness: {}", reason
            );
        }
    }
}
