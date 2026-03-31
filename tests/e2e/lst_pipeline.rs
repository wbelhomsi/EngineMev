//! E2E tests for LST arb pipeline using Surfpool.
//!
//! These tests require a running Surfpool instance:
//!   NO_DNA=1 surfpool start --ci --network mainnet
//!
//! Run with: cargo test --features e2e --test e2e -- --test-threads=1

use solana_sdk::pubkey::Pubkey;
use std::time::Duration;

use solana_mev_bot::config;
use solana_mev_bot::mempool::PoolStateChange;
use solana_mev_bot::router::pool::{DexType, DetectedSwap, PoolState};
use solana_mev_bot::router::{RouteCalculator, ProfitSimulator};
use solana_mev_bot::state::StateCache;

/// Helper: set up a StateCache with Orca and Sanctum pools for jitoSOL/SOL
/// with a known spread.
fn setup_cache_with_spread(orca_rate: f64, sanctum_rate: f64) -> (StateCache, Pubkey, Pubkey) {
    let cache = StateCache::new(Duration::from_secs(60));
    let sol = config::sol_mint();
    let jitosol = config::lst_mints()[0].0;

    let orca_addr = Pubkey::new_unique();
    let sanctum_addr = Pubkey::new_unique();

    // Orca pool — 100K SOL for reasonable auto-input (1% = 1K SOL)
    let orca_sol_reserve = 100_000_000_000_000u64;
    let orca_jitosol_reserve = (orca_sol_reserve as f64 / orca_rate) as u64;
    cache.upsert(orca_addr, PoolState {
        address: orca_addr,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: sol,
        token_b_mint: jitosol,
        token_a_reserve: orca_sol_reserve,
        token_b_reserve: orca_jitosol_reserve,
        fee_bps: 1,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
    });

    // Sanctum virtual pool
    let reserve_base: u64 = 1_000_000_000_000_000;
    cache.upsert(sanctum_addr, PoolState {
        address: sanctum_addr,
        dex_type: DexType::SanctumInfinity,
        token_a_mint: sol,
        token_b_mint: jitosol,
        token_a_reserve: reserve_base,
        token_b_reserve: (reserve_base as f64 / sanctum_rate) as u64,
        fee_bps: 3,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
    });

    (cache, orca_addr, sanctum_addr)
}

#[test]
fn test_e2e_profitable_arb_pipeline() {
    // Orca rate 1.050, Sanctum rate 1.082 -> ~3% spread -> profitable after fees
    let (cache, orca_addr, _sanctum_addr) = setup_cache_with_spread(1.050, 1.082);

    let calculator = RouteCalculator::new(cache.clone(), 3);
    let simulator = ProfitSimulator::new(cache.clone(), 0.50, 1000);

    // Simulate Geyser event: vault balance changed on Orca pool.
    // Search both directions like main.rs does.
    let trigger_fwd = DetectedSwap {
        signature: String::new(),
        dex_type: DexType::OrcaWhirlpool,
        pool_address: orca_addr,
        input_mint: config::sol_mint(),
        output_mint: config::lst_mints()[0].0,
        amount: None,
        observed_slot: 100,
    };
    let trigger_rev = DetectedSwap {
        signature: String::new(),
        dex_type: DexType::OrcaWhirlpool,
        pool_address: orca_addr,
        input_mint: config::lst_mints()[0].0,
        output_mint: config::sol_mint(),
        amount: None,
        observed_slot: 100,
    };

    // Route discovery
    let mut routes = calculator.find_routes(&trigger_fwd);
    routes.extend(calculator.find_routes(&trigger_rev));
    routes.sort_by(|a, b| b.estimated_profit.cmp(&a.estimated_profit));
    assert!(!routes.is_empty(), "Should find arb routes");

    // Simulation
    let best = &routes[0];
    let result = simulator.simulate(best);
    match result {
        solana_mev_bot::router::simulator::SimulationResult::Profitable {
            final_profit_lamports,
            tip_lamports,
            ..
        } => {
            assert!(final_profit_lamports > 0, "Positive profit");
            assert!(tip_lamports > 0, "Non-zero tip");
        }
        solana_mev_bot::router::simulator::SimulationResult::Unprofitable { reason } => {
            panic!("Expected profitable: {}", reason);
        }
    }
}

#[test]
fn test_e2e_revert_unprofitable() {
    // Same rate on both pools -> fees make it unprofitable
    let (cache, orca_addr, _sanctum_addr) = setup_cache_with_spread(1.082, 1.082);

    let calculator = RouteCalculator::new(cache.clone(), 3);
    let simulator = ProfitSimulator::new(cache.clone(), 0.50, 1000);

    let trigger = DetectedSwap {
        signature: String::new(),
        dex_type: DexType::OrcaWhirlpool,
        pool_address: orca_addr,
        input_mint: config::sol_mint(),
        output_mint: config::lst_mints()[0].0,
        amount: None,
        observed_slot: 100,
    };

    let routes = calculator.find_routes(&trigger);
    // Either no routes found, or all are unprofitable after simulation
    for route in &routes {
        let result = simulator.simulate(route);
        match result {
            solana_mev_bot::router::simulator::SimulationResult::Unprofitable { .. } => {
                // Expected
            }
            solana_mev_bot::router::simulator::SimulationResult::Profitable { .. } => {
                panic!("Should NOT be profitable when rates are equal");
            }
        }
    }
}

#[test]
fn test_e2e_stale_state_rejected() {
    let (cache, orca_addr, _) = setup_cache_with_spread(1.075, 1.082);

    let orca_vault = Pubkey::new_unique();
    // Register a vault for the orca pool
    cache.register_vault(orca_vault, orca_addr, true);

    // Apply an update at slot 100
    cache.update_vault_balance(&orca_vault, 10_000_000_000_000, 100);

    // Try to apply a stale update at slot 50 — should be ignored
    let result = cache.update_vault_balance(&orca_vault, 5_000_000_000_000, 50);
    assert!(result.is_none(), "Stale update (slot 50 < 100) should be rejected");

    // Verify the reserve didn't change
    let pool = cache.get_any(&orca_addr).unwrap();
    assert_eq!(pool.token_a_reserve, 10_000_000_000_000, "Reserve should be unchanged after stale update");
}

#[test]
fn test_e2e_channel_backpressure() {
    use crossbeam_channel::bounded;

    let (tx, rx) = bounded::<PoolStateChange>(2); // tiny capacity

    // Fill the channel
    let change1 = PoolStateChange { vault_address: Pubkey::new_unique(), new_balance: 100, slot: 1 };
    let change2 = PoolStateChange { vault_address: Pubkey::new_unique(), new_balance: 200, slot: 2 };
    let change3 = PoolStateChange { vault_address: Pubkey::new_unique(), new_balance: 300, slot: 3 };

    assert!(tx.try_send(change1).is_ok());
    assert!(tx.try_send(change2).is_ok());
    // Channel full — try_send should fail (not block)
    assert!(tx.try_send(change3).is_err(), "try_send should fail on full channel, not block");

    // Drain and verify we got the first two
    let c1 = rx.try_recv().unwrap();
    assert_eq!(c1.slot, 1);
    let c2 = rx.try_recv().unwrap();
    assert_eq!(c2.slot, 2);
}
