use solana_sdk::pubkey::Pubkey;
use std::time::Duration;

use solana_mev_bot::config;
use solana_mev_bot::router::pool::{ArbRoute, DexType, PoolExtra, PoolState, RouteHop};
use solana_mev_bot::router::ProfitSimulator;
use solana_mev_bot::state::StateCache;

fn sol_mint() -> Pubkey {
    config::sol_mint()
}

fn jitosol_mint() -> Pubkey {
    config::lst_mints()[0].0
}

fn make_cache_with_pools(orca_addr: Pubkey, sanctum_addr: Pubkey) -> StateCache {
    let cache = StateCache::new(Duration::from_secs(60));

    // Orca pool: rate ~1.075 (cheap jitoSOL)
    cache.upsert(orca_addr, PoolState {
        address: orca_addr,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: sol_mint(),
        token_b_mint: jitosol_mint(),
        token_a_reserve: 10_000_000_000_000,
        token_b_reserve: 9_302_325_581_395,
        fee_bps: 1,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    });

    // Sanctum virtual pool: rate 1.082
    let reserve_a: u64 = 1_000_000_000_000_000;
    cache.upsert(sanctum_addr, PoolState {
        address: sanctum_addr,
        dex_type: DexType::SanctumInfinity,
        token_a_mint: sol_mint(),
        token_b_mint: jitosol_mint(),
        token_a_reserve: reserve_a,
        token_b_reserve: (reserve_a as f64 / 1.082) as u64,
        fee_bps: 3,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    });

    cache
}

#[test]
fn test_simulator_approves_profitable_lst_route() {
    let orca_addr = Pubkey::new_unique();
    let sanctum_addr = Pubkey::new_unique();
    let cache = make_cache_with_pools(orca_addr, sanctum_addr);

    let simulator = ProfitSimulator::new(cache, 0.50, 1000); // 50% tip, 1000 lamport min

    let route = ArbRoute {
        hops: vec![
            RouteHop {
                pool_address: orca_addr,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol_mint(),
                output_mint: jitosol_mint(),
                estimated_output: 9_000_000,
            },
            RouteHop {
                pool_address: sanctum_addr,
                dex_type: DexType::SanctumInfinity,
                input_mint: jitosol_mint(),
                output_mint: sol_mint(),
                estimated_output: 10_100_000,
            },
        ],
        base_mint: sol_mint(),
        input_amount: 10_000_000, // 0.01 SOL
        estimated_profit: 100_000,
        estimated_profit_lamports: 100_000,
    };

    let result = simulator.simulate(&route);
    match result {
        solana_mev_bot::router::simulator::SimulationResult::Profitable { final_profit_lamports, .. } => {
            assert!(final_profit_lamports > 0, "Should have positive final profit");
        }
        solana_mev_bot::router::simulator::SimulationResult::Unprofitable { reason } => {
            panic!("Expected profitable, got: {}", reason);
        }
    }
}

#[test]
fn test_simulator_rejects_below_min_profit() {
    let orca_addr = Pubkey::new_unique();
    let sanctum_addr = Pubkey::new_unique();
    let cache = make_cache_with_pools(orca_addr, sanctum_addr);

    // Set min profit very high — route should be rejected
    let simulator = ProfitSimulator::new(cache, 0.50, 999_999_999_999);

    let route = ArbRoute {
        hops: vec![
            RouteHop {
                pool_address: orca_addr,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol_mint(),
                output_mint: jitosol_mint(),
                estimated_output: 9_000_000,
            },
            RouteHop {
                pool_address: sanctum_addr,
                dex_type: DexType::SanctumInfinity,
                input_mint: jitosol_mint(),
                output_mint: sol_mint(),
                estimated_output: 10_100_000,
            },
        ],
        base_mint: sol_mint(),
        input_amount: 10_000_000,
        estimated_profit: 100_000,
        estimated_profit_lamports: 100_000,
    };

    let result = simulator.simulate(&route);
    match result {
        solana_mev_bot::router::simulator::SimulationResult::Unprofitable { reason } => {
            assert!(reason.contains("Below minimum"), "Should reject: {}", reason);
        }
        _ => panic!("Expected Unprofitable"),
    }
}

#[test]
fn test_simulator_rejects_when_total_tips_exceed_profit() {
    let orca_addr = Pubkey::new_unique();
    let sanctum_addr = Pubkey::new_unique();
    let cache = make_cache_with_pools(orca_addr, sanctum_addr);

    // Route produces ~64,730 lamports gross profit (verified from pool math)
    // 50% tip fraction = ~32,365 Jito tip
    // Add 100,000 relay extra tips => total ~132,365 > 64,730 gross profit
    // This should be rejected as "would lose money"
    let simulator = ProfitSimulator::new(cache, 0.50, 1000)
        .with_relay_extra_tips(100_000);

    let route = ArbRoute {
        hops: vec![
            RouteHop {
                pool_address: orca_addr,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol_mint(),
                output_mint: jitosol_mint(),
                estimated_output: 9_000_000,
            },
            RouteHop {
                pool_address: sanctum_addr,
                dex_type: DexType::SanctumInfinity,
                input_mint: jitosol_mint(),
                output_mint: sol_mint(),
                estimated_output: 10_100_000,
            },
        ],
        base_mint: sol_mint(),
        input_amount: 10_000_000,
        estimated_profit: 100_000,
        estimated_profit_lamports: 100_000,
    };

    let result = simulator.simulate(&route);
    match result {
        solana_mev_bot::router::simulator::SimulationResult::Unprofitable { reason } => {
            assert!(reason.contains("would lose money"), "Should reject due to tips exceeding profit: {}", reason);
        }
        _ => panic!("Expected Unprofitable when relay tips exceed profit"),
    }
}

#[test]
fn test_simulator_writes_fresh_hop_outputs() {
    let orca_addr = Pubkey::new_unique();
    let sanctum_addr = Pubkey::new_unique();
    let cache = make_cache_with_pools(orca_addr, sanctum_addr);

    let simulator = ProfitSimulator::new(cache, 0.50, 1000);

    // Use intentionally stale estimated_output values
    let route = ArbRoute {
        hops: vec![
            RouteHop {
                pool_address: orca_addr,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol_mint(),
                output_mint: jitosol_mint(),
                estimated_output: 1, // stale — should be overwritten
            },
            RouteHop {
                pool_address: sanctum_addr,
                dex_type: DexType::SanctumInfinity,
                input_mint: jitosol_mint(),
                output_mint: sol_mint(),
                estimated_output: 1, // stale — should be overwritten
            },
        ],
        base_mint: sol_mint(),
        input_amount: 10_000_000,
        estimated_profit: 100_000,
        estimated_profit_lamports: 100_000,
    };

    let result = simulator.simulate(&route);
    match result {
        solana_mev_bot::router::simulator::SimulationResult::Profitable { route: fresh_route, .. } => {
            // Fresh outputs should be non-trivial (not the stale value of 1)
            assert!(fresh_route.hops[0].estimated_output > 1,
                "Hop 0 estimated_output should be freshly computed, got {}",
                fresh_route.hops[0].estimated_output);
            assert!(fresh_route.hops[1].estimated_output > 1,
                "Hop 1 estimated_output should be freshly computed, got {}",
                fresh_route.hops[1].estimated_output);
            // Final hop output should be greater than input (profitable route)
            assert!(fresh_route.hops[1].estimated_output > route.input_amount,
                "Final output {} should exceed input {}",
                fresh_route.hops[1].estimated_output, route.input_amount);
        }
        solana_mev_bot::router::simulator::SimulationResult::Unprofitable { reason } => {
            panic!("Expected profitable, got: {}", reason);
        }
    }
}

#[test]
fn test_simulator_total_tips_includes_relay_extras() {
    let orca_addr = Pubkey::new_unique();
    let sanctum_addr = Pubkey::new_unique();
    let cache = make_cache_with_pools(orca_addr, sanctum_addr);

    // Use a large input so profit is high enough to survive all tips
    let big_cache = StateCache::new(Duration::from_secs(60));

    // Pools with enough spread to generate >200k profit on 1 SOL input
    big_cache.upsert(orca_addr, PoolState {
        address: orca_addr,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: sol_mint(),
        token_b_mint: jitosol_mint(),
        token_a_reserve: 10_000_000_000_000,
        token_b_reserve: 9_302_325_581_395,
        fee_bps: 1,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    });

    big_cache.upsert(sanctum_addr, PoolState {
        address: sanctum_addr,
        dex_type: DexType::SanctumInfinity,
        token_a_mint: sol_mint(),
        token_b_mint: jitosol_mint(),
        token_a_reserve: 1_000_000_000_000_000,
        token_b_reserve: (1_000_000_000_000_000_f64 / 1.082) as u64,
        fee_bps: 3,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    });

    let relay_extra = 50_000;
    let simulator = ProfitSimulator::new(big_cache, 0.10, 1000) // low tip fraction
        .with_relay_extra_tips(relay_extra);

    let route = ArbRoute {
        hops: vec![
            RouteHop {
                pool_address: orca_addr,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol_mint(),
                output_mint: jitosol_mint(),
                estimated_output: 9_000_000_000,
            },
            RouteHop {
                pool_address: sanctum_addr,
                dex_type: DexType::SanctumInfinity,
                input_mint: jitosol_mint(),
                output_mint: sol_mint(),
                estimated_output: 10_100_000_000,
            },
        ],
        base_mint: sol_mint(),
        input_amount: 1_000_000_000, // 1 SOL
        estimated_profit: 100_000_000,
        estimated_profit_lamports: 100_000_000,
    };

    let result = simulator.simulate(&route);
    match result {
        solana_mev_bot::router::simulator::SimulationResult::Profitable {
            total_tip_lamports,
            net_profit_lamports,
            final_profit_lamports,
            ..
        } => {
            // total_tip should include relay extra
            let jito_portion = (net_profit_lamports as f64 * 0.10) as u64;
            assert_eq!(total_tip_lamports, jito_portion + relay_extra,
                "total_tip should be jito({}) + relay_extra({}), got {}",
                jito_portion, relay_extra, total_tip_lamports);
            // final profit should be gross - total_tips
            assert_eq!(final_profit_lamports, net_profit_lamports - total_tip_lamports);
        }
        solana_mev_bot::router::simulator::SimulationResult::Unprofitable { reason } => {
            panic!("Expected profitable, got: {}", reason);
        }
    }
}
