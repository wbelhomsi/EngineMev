use std::time::Duration;

use solana_sdk::pubkey::Pubkey;

use solana_mev_bot::config;
use solana_mev_bot::router::pool::{ArbRoute, DexType, PoolExtra, PoolState, RouteHop};
use solana_mev_bot::router::simulator::SimulationResult;
use solana_mev_bot::router::ProfitSimulator;
use solana_mev_bot::state::StateCache;

/// Verify that the simulator rejects routes with unrealistically high profit.
/// A route showing > 10 SOL profit from a single CPMM arb is almost certainly
/// an approximation artifact from stale reserves.
#[test]
fn test_simulator_rejects_unrealistic_profit() {
    let cache = StateCache::new(Duration::from_secs(60));
    let sol = config::sol_mint();
    let token = Pubkey::new_unique();

    // Pool A: SOL/TOKEN with normal ratio
    let pool_a = Pubkey::new_unique();
    cache.upsert(
        pool_a,
        PoolState {
            address: pool_a,
            dex_type: DexType::RaydiumCp,
            token_a_mint: sol,
            token_b_mint: token,
            token_a_reserve: 1_000_000_000_000_000,   // 1M SOL
            token_b_reserve: 10_000_000_000_000_000,   // 10M tokens
            fee_bps: 25,
            current_tick: None,
            sqrt_price_x64: None,
            liquidity: None,
            last_slot: 100,
            extra: PoolExtra::default(),
            best_bid_price: None,
            best_ask_price: None,
        },
    );

    // Pool B: TOKEN/SOL with grossly stale state — tokens worth 3x more SOL
    let pool_b = Pubkey::new_unique();
    cache.upsert(
        pool_b,
        PoolState {
            address: pool_b,
            dex_type: DexType::RaydiumCp,
            token_a_mint: token,
            token_b_mint: sol,
            token_a_reserve: 3_300_000_000_000_000,   // 3.3M tokens
            token_b_reserve: 1_000_000_000_000_000,   // 1M SOL
            fee_bps: 25,
            current_tick: None,
            sqrt_price_x64: None,
            liquidity: None,
            last_slot: 100,
            extra: PoolExtra::default(),
            best_bid_price: None,
            best_ask_price: None,
        },
    );

    let simulator = ProfitSimulator::new(cache, 0.50, 1000);

    let route = ArbRoute {
        base_mint: sol,
        input_amount: 100_000_000_000, // 100 SOL input
        estimated_profit: 50_000_000_000, // claims 50 SOL profit
        estimated_profit_lamports: 50_000_000_000,
        hops: vec![
            RouteHop {
                pool_address: pool_a,
                dex_type: DexType::RaydiumCp,
                input_mint: sol,
                output_mint: token,
                estimated_output: 0,
            },
            RouteHop {
                pool_address: pool_b,
                dex_type: DexType::RaydiumCp,
                input_mint: token,
                output_mint: sol,
                estimated_output: 150_000_000_000, // claims 150 SOL out from 100 SOL in
            },
        ],
    };

    let result = simulator.simulate(&route);
    match result {
        SimulationResult::Unprofitable { reason } => {
            assert!(
                reason.contains("sanity") || reason.contains("cap"),
                "Should be rejected by sanity cap, got: {}",
                reason
            );
        }
        SimulationResult::Profitable {
            final_profit_lamports,
            ..
        } => {
            assert!(
                final_profit_lamports <= 10_000_000_000,
                "Profit {} should be capped at 10 SOL",
                final_profit_lamports
            );
        }
    }
}

/// Verify that legitimate small profits still pass through the simulator
/// without being blocked by the sanity cap.
#[test]
fn test_simulator_allows_small_realistic_profit() {
    let cache = StateCache::new(Duration::from_secs(60));
    let sol = config::sol_mint();
    let token = Pubkey::new_unique();

    // Two pools with a small realistic spread (~0.5%)
    let pool_a = Pubkey::new_unique();
    cache.upsert(
        pool_a,
        PoolState {
            address: pool_a,
            dex_type: DexType::OrcaWhirlpool,
            token_a_mint: sol,
            token_b_mint: token,
            token_a_reserve: 10_000_000_000_000, // 10K SOL
            token_b_reserve: 10_000_000_000_000,
            fee_bps: 25,
            current_tick: None,
            sqrt_price_x64: None,
            liquidity: None,
            last_slot: 100,
            extra: PoolExtra::default(),
            best_bid_price: None,
            best_ask_price: None,
        },
    );

    let pool_b = Pubkey::new_unique();
    cache.upsert(
        pool_b,
        PoolState {
            address: pool_b,
            dex_type: DexType::RaydiumCp,
            token_a_mint: sol,
            token_b_mint: token,
            token_a_reserve: 10_000_000_000_000,
            token_b_reserve: 10_050_000_000_000, // 0.5% more token
            fee_bps: 25,
            current_tick: None,
            sqrt_price_x64: None,
            liquidity: None,
            last_slot: 100,
            extra: PoolExtra::default(),
            best_bid_price: None,
            best_ask_price: None,
        },
    );

    let simulator = ProfitSimulator::new(cache, 0.50, 1000);

    let route = ArbRoute {
        base_mint: sol,
        input_amount: 10_000_000, // 0.01 SOL
        estimated_profit: 50_000,
        estimated_profit_lamports: 50_000,
        hops: vec![
            RouteHop {
                pool_address: pool_a,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol,
                output_mint: token,
                estimated_output: 0,
            },
            RouteHop {
                pool_address: pool_b,
                dex_type: DexType::RaydiumCp,
                input_mint: token,
                output_mint: sol,
                estimated_output: 10_050_000,
            },
        ],
    };

    let result = simulator.simulate(&route);
    // This should NOT be rejected by sanity cap
    match result {
        SimulationResult::Profitable { .. } => { /* good */ }
        SimulationResult::Unprofitable { reason } => {
            // OK if unprofitable for legitimate reasons (fees, etc.)
            // but should NOT mention "sanity cap"
            assert!(
                !reason.contains("sanity cap"),
                "Small profit should not hit sanity cap: {}",
                reason
            );
        }
    }
}
