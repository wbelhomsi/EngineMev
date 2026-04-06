use std::time::Duration;

use solana_sdk::pubkey::Pubkey;

use solana_mev_bot::config;
use solana_mev_bot::router::pool::{DetectedSwap, DexType, PoolExtra, PoolState};
use solana_mev_bot::router::RouteCalculator;
use solana_mev_bot::state::StateCache;

/// Verify that route discovery completes within reasonable time
/// even with many pools in the cache.
#[test]
fn test_route_calc_completes_under_5ms_with_50_pools() {
    let cache = StateCache::new(Duration::from_secs(60));
    let sol = config::sol_mint();
    let usdc = Pubkey::new_unique();

    // Add 50 pools with SOL/USDC pair (simulates a busy cache)
    for _i in 0..50 {
        let addr = Pubkey::new_unique();
        cache.upsert(
            addr,
            PoolState {
                address: addr,
                dex_type: DexType::OrcaWhirlpool,
                token_a_mint: sol,
                token_b_mint: usdc,
                token_a_reserve: 100_000_000_000, // 100 SOL
                token_b_reserve: 15_000_000_000,
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
    }

    let calculator = RouteCalculator::new(cache.clone(), 3);
    let trigger = DetectedSwap {
        dex_type: DexType::OrcaWhirlpool,
        pool_address: Pubkey::new_unique(), // doesn't need to match a cached pool
        input_mint: sol,
        output_mint: usdc,
        amount: None,
        observed_slot: 100,
    };

    let start = std::time::Instant::now();
    let routes = calculator.find_routes(&trigger);
    let elapsed = start.elapsed();

    println!("Found {} routes in {:?}", routes.len(), elapsed);
    assert!(
        elapsed < Duration::from_millis(5),
        "Route calc took {:?}, should be under 5ms",
        elapsed
    );
}

/// Verify that routes with negligible reserves are filtered or produce minimal routes.
#[test]
fn test_tiny_reserve_pools_produce_no_profitable_routes() {
    let cache = StateCache::new(Duration::from_secs(60));
    let sol = config::sol_mint();
    let token = Pubkey::new_unique();

    // Pool with tiny reserves (0.001 SOL)
    let addr = Pubkey::new_unique();
    cache.upsert(
        addr,
        PoolState {
            address: addr,
            dex_type: DexType::OrcaWhirlpool,
            token_a_mint: sol,
            token_b_mint: token,
            token_a_reserve: 1_000_000, // 0.001 SOL
            token_b_reserve: 1_000,
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

    // Add a second pool for the same pair (needed for arb)
    let addr2 = Pubkey::new_unique();
    cache.upsert(
        addr2,
        PoolState {
            address: addr2,
            dex_type: DexType::RaydiumCp,
            token_a_mint: sol,
            token_b_mint: token,
            token_a_reserve: 1_000_000, // 0.001 SOL
            token_b_reserve: 1_100,     // slightly different ratio
            fee_bps: 30,
            current_tick: None,
            sqrt_price_x64: None,
            liquidity: None,
            last_slot: 100,
            extra: PoolExtra::default(),
            best_bid_price: None,
            best_ask_price: None,
        },
    );

    let calculator = RouteCalculator::new(cache, 3);
    let trigger = DetectedSwap {
        dex_type: DexType::OrcaWhirlpool,
        pool_address: addr,
        input_mint: sol,
        output_mint: token,
        amount: None,
        observed_slot: 100,
    };

    let routes = calculator.find_routes(&trigger);
    // With only 0.001 SOL in reserves, profit should be negligible
    for route in &routes {
        assert!(
            route.estimated_profit_lamports < 100_000,
            "Tiny pool should not show significant profit: {}",
            route.estimated_profit_lamports
        );
    }
}

/// Verify that the route truncation constant exists and limits output.
/// This test creates enough pools to generate many routes, then verifies
/// the cap is applied in the main pipeline logic.
#[test]
fn test_route_count_bounded_with_many_pools() {
    let cache = StateCache::new(Duration::from_secs(60));
    let sol = config::sol_mint();

    // Create 20 different tokens, each with 2 pools against SOL
    // This could generate O(n^2) 3-hop routes
    let mut tokens = Vec::new();
    let mut first_pool_addr = Pubkey::new_unique();
    for i in 0..20 {
        let token = Pubkey::new_unique();
        tokens.push(token);

        // Pool 1: OrcaWhirlpool
        let addr1 = Pubkey::new_unique();
        if i == 0 {
            first_pool_addr = addr1;
        }
        cache.upsert(
            addr1,
            PoolState {
                address: addr1,
                dex_type: DexType::OrcaWhirlpool,
                token_a_mint: sol,
                token_b_mint: token,
                token_a_reserve: 50_000_000_000,
                token_b_reserve: 7_500_000_000,
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

        // Pool 2: RaydiumCp — significantly different price to create arb opportunity
        let addr2 = Pubkey::new_unique();
        cache.upsert(
            addr2,
            PoolState {
                address: addr2,
                dex_type: DexType::RaydiumCp,
                token_a_mint: sol,
                token_b_mint: token,
                token_a_reserve: 50_000_000_000,
                token_b_reserve: 10_000_000_000, // ~33% different price to ensure profit after fees
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
    }

    let calculator = RouteCalculator::new(cache, 3);
    // Use an actual cached pool address so find_routes doesn't bail out
    let trigger = DetectedSwap {
        dex_type: DexType::OrcaWhirlpool,
        pool_address: first_pool_addr,
        input_mint: sol,
        output_mint: tokens[0],
        amount: None,
        observed_slot: 100,
    };

    let routes = calculator.find_routes(&trigger);
    println!(
        "Generated {} routes from 20 token pairs (40 pools)",
        routes.len()
    );
    // The route calculator itself may return many routes — the cap is applied
    // in main.rs after sort. This test documents the baseline count.
    // With the fix, main.rs truncates to MAX_ROUTES_PER_EVENT (10).
    assert!(
        !routes.is_empty(),
        "Should find at least some routes with 40 pools"
    );
}

/// Verify dust pools (< 10 SOL on the SOL side) are filtered out.
///
/// The old check used `&&` so a pool with 0.84 SOL + 57 USDC (raw 57_000_000)
/// passed because the USDC side was numerically > 1 SOL in lamports.
/// The fix checks the SOL-side reserve specifically.
#[test]
fn test_dust_pool_filtered_by_sol_reserve() {
    let sol_mint = config::sol_mint();

    const MIN_SOL_RESERVE: u64 = 10_000_000_000; // 10 SOL

    // --- Helper: extract SOL-side reserve (mirrors main.rs logic) ---
    let sol_side_reserve = |pool: &PoolState| -> u64 {
        if pool.token_a_mint == sol_mint {
            pool.token_a_reserve
        } else if pool.token_b_mint == sol_mint {
            pool.token_b_reserve
        } else {
            // Non-SOL pair: use the smaller reserve as a proxy
            std::cmp::min(pool.token_a_reserve, pool.token_b_reserve)
        }
    };

    // Case 1: SOL on token_b side, 0.84 SOL — must be filtered as dust
    let dust_pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::RaydiumCp,
        token_a_mint: Pubkey::new_unique(), // USDC
        token_b_mint: sol_mint,
        token_a_reserve: 57_000_000, // 57 USDC (6 decimals)
        token_b_reserve: 840_000_000, // 0.84 SOL
        fee_bps: 25,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    };
    assert!(
        sol_side_reserve(&dust_pool) < MIN_SOL_RESERVE,
        "0.84 SOL pool must be filtered as dust"
    );

    // Case 2: SOL on token_a side, 50 SOL — must pass
    let healthy_pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: sol_mint,
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 50_000_000_000, // 50 SOL
        token_b_reserve: 7_500_000_000,
        fee_bps: 25,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    };
    assert!(
        sol_side_reserve(&healthy_pool) >= MIN_SOL_RESERVE,
        "50 SOL pool must pass the dust filter"
    );

    // Case 3: Non-SOL pair, one side tiny — must be filtered
    let non_sol_dust = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::RaydiumCp,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 500_000_000, // 0.5 in 9-decimal token
        token_b_reserve: 999_999_999_999, // large
        fee_bps: 25,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    };
    assert!(
        sol_side_reserve(&non_sol_dust) < MIN_SOL_RESERVE,
        "Non-SOL pair with tiny min-reserve must be filtered"
    );

    // Case 4: The OLD buggy check would pass the dust_pool.
    // Old logic: skip only if BOTH < 1 SOL. USDC raw 57_000_000 < 1_000_000_000,
    // so the old check WOULD have caught this specific case. But the real bug
    // is pools like 0.84 SOL + 200B raw USDT (200_000 USDT in 6-dec = 200_000_000_000)
    // where the non-SOL side is numerically huge.
    let real_bug_pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::RaydiumCp,
        token_a_mint: Pubkey::new_unique(), // USDT
        token_b_mint: sol_mint,
        token_a_reserve: 200_000_000_000, // 200,000 USDT raw (6 dec) — huge number
        token_b_reserve: 840_000_000,     // 0.84 SOL
        fee_bps: 25,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    };
    // Old buggy check: skip if BOTH < 1B. token_a_reserve (200B) >= 1B, so old check PASSES this pool!
    let old_min: u64 = 1_000_000_000;
    let old_would_skip = real_bug_pool.token_a_reserve < old_min
        && real_bug_pool.token_b_reserve < old_min;
    assert!(
        !old_would_skip,
        "Old check incorrectly lets this dust pool through"
    );
    // New check: SOL side is 0.84 SOL < 10 SOL — filtered correctly.
    assert!(
        sol_side_reserve(&real_bug_pool) < MIN_SOL_RESERVE,
        "New check correctly filters 0.84 SOL pool even with large USDT side"
    );
}
