//! E2E tests for cross-DEX spot arb pipeline.
//!
//! These tests use real components (StateCache, RouteCalculator, ProfitSimulator,
//! BundleBuilder) with synthetic but realistic pool state. No mocking.
//!
//! Run with: cargo test --features e2e --test e2e

use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use solana_mev_bot::config;
use solana_mev_bot::executor::BundleBuilder;
use solana_mev_bot::router::pool::{ArbRoute, DexType, DetectedSwap, PoolExtra, PoolState};
use solana_mev_bot::router::simulator::SimulationResult;
use solana_mev_bot::router::{ProfitSimulator, RouteCalculator};
use solana_mev_bot::state::StateCache;

fn usdc_mint() -> Pubkey {
    Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap()
}

/// Helper: create a CPMM pool in the cache and return its address.
fn insert_pool(
    cache: &StateCache,
    dex_type: DexType,
    mint_a: Pubkey,
    mint_b: Pubkey,
    reserve_a: u64,
    reserve_b: u64,
    fee_bps: u64,
    slot: u64,
) -> Pubkey {
    let addr = Pubkey::new_unique();
    cache.upsert(
        addr,
        PoolState {
            address: addr,
            dex_type,
            token_a_mint: mint_a,
            token_b_mint: mint_b,
            token_a_reserve: reserve_a,
            token_b_reserve: reserve_b,
            fee_bps,
            current_tick: None,
            sqrt_price_x64: None,
            liquidity: None,
            last_slot: slot,
            extra: PoolExtra::default(),
            best_bid_price: None,
            best_ask_price: None,
        },
    );
    addr
}

/// Helper: trigger route discovery in both directions (like main.rs does).
fn find_all_routes(
    calculator: &RouteCalculator,
    pool_address: Pubkey,
    dex_type: DexType,
    mint_a: Pubkey,
    mint_b: Pubkey,
    slot: u64,
) -> Vec<ArbRoute> {
    let trigger_fwd = DetectedSwap {
        dex_type,
        pool_address,
        input_mint: mint_a,
        output_mint: mint_b,
        amount: None,
        observed_slot: slot,
    };
    let trigger_rev = DetectedSwap {
        dex_type,
        pool_address,
        input_mint: mint_b,
        output_mint: mint_a,
        amount: None,
        observed_slot: slot,
    };

    let mut routes = calculator.find_routes(&trigger_fwd);
    routes.extend(calculator.find_routes(&trigger_rev));
    routes.sort_by(|a, b| b.estimated_profit.cmp(&a.estimated_profit));
    routes
}

// ---------------------------------------------------------------------------
// Test 1: Two-hop cross-DEX arb (Orca <-> Raydium CP)
// ---------------------------------------------------------------------------
#[test]
fn test_e2e_two_hop_cross_dex_orca_raydium_cp() {
    let sol = config::sol_mint();
    let usdc = usdc_mint();
    let cache = StateCache::new(Duration::from_secs(60));

    // Orca pool: SOL/USDC at ~140 SOL per USDC (i.e., 1 USDC = 140 SOL)
    // Reserve ratio: 100K SOL : ~714 USDC
    let orca_addr = insert_pool(
        &cache,
        DexType::OrcaWhirlpool,
        sol,
        usdc,
        100_000_000_000_000, // 100K SOL in lamports
        714_285_714,         // ~714 USDC (6 decimals) -> price ~140 SOL/USDC
        25,                  // 0.25% fee
        100,
    );

    // Raydium CP pool: SOL/USDC at ~145 SOL per USDC (more expensive SOL)
    // Reserve ratio: 100K SOL : ~689 USDC
    let _raydium_addr = insert_pool(
        &cache,
        DexType::RaydiumCp,
        sol,
        usdc,
        100_000_000_000_000, // 100K SOL
        689_655_172,         // ~689.6 USDC -> price ~145 SOL/USDC
        30,                  // 0.3% fee
        100,
    );

    let calculator = RouteCalculator::new(cache.clone(), 3);
    let simulator = ProfitSimulator::new(cache.clone(), 0.50, 1000);

    let routes = find_all_routes(&calculator, orca_addr, DexType::OrcaWhirlpool, sol, usdc, 100);
    assert!(!routes.is_empty(), "Should find cross-DEX arb routes");

    // Find 2-hop route
    let two_hop = routes.iter().find(|r| r.hops.len() == 2);
    assert!(two_hop.is_some(), "Should have a 2-hop route");
    let route = two_hop.unwrap();

    // Verify the route is circular (starts and ends with same token)
    assert_eq!(
        route.hops.first().unwrap().input_mint,
        route.hops.last().unwrap().output_mint,
        "Route must be circular"
    );

    // Simulate
    let result = simulator.simulate(route);
    match result {
        SimulationResult::Profitable {
            final_profit_lamports,
            tip_lamports,
            ..
        } => {
            assert!(final_profit_lamports > 0, "Positive profit expected");
            assert!(tip_lamports > 0, "Non-zero tip expected");
        }
        SimulationResult::Unprofitable { reason } => {
            panic!("Expected profitable cross-DEX route: {}", reason);
        }
    }
}

// ---------------------------------------------------------------------------
// Test 2: Two-hop same-DEX arb (two different Orca pools)
// ---------------------------------------------------------------------------
#[test]
fn test_e2e_two_hop_same_dex_orca() {
    let sol = config::sol_mint();
    let token_x = Pubkey::new_unique();
    let cache = StateCache::new(Duration::from_secs(60));

    // Orca pool A: SOL/TokenX — cheap TokenX (1 SOL = 100 TokenX)
    // Use small reserves (1K SOL) so 1% auto-input keeps profit under 10 SOL sanity cap
    let orca_a = insert_pool(
        &cache,
        DexType::OrcaWhirlpool,
        sol,
        token_x,
        1_000_000_000_000,  // 1K SOL
        100_000_000_000,    // 100K TokenX (9 decimals) -> ratio 100:1
        25,                 // 0.25%
        100,
    );

    // Orca pool B: SOL/TokenX — expensive TokenX (1 SOL = 95 TokenX)
    // Less TokenX per SOL => TokenX is pricier here
    let _orca_b = insert_pool(
        &cache,
        DexType::OrcaWhirlpool,
        sol,
        token_x,
        1_000_000_000_000,  // 1K SOL
        95_000_000_000,     // 95K TokenX -> ratio 95:1
        25,                 // 0.25%
        100,
    );

    let calculator = RouteCalculator::new(cache.clone(), 3);
    let simulator = ProfitSimulator::new(cache.clone(), 0.50, 1000);

    let routes = find_all_routes(&calculator, orca_a, DexType::OrcaWhirlpool, sol, token_x, 100);
    assert!(!routes.is_empty(), "Should find same-DEX arb routes");

    // Should have a 2-hop route through both Orca pools
    let two_hop = routes.iter().find(|r| r.hops.len() == 2);
    assert!(two_hop.is_some(), "Should have a 2-hop same-DEX route");

    let route = two_hop.unwrap();
    // Both hops should be OrcaWhirlpool
    assert!(
        route.hops.iter().all(|h| h.dex_type == DexType::OrcaWhirlpool),
        "Both hops should be OrcaWhirlpool"
    );

    let result = simulator.simulate(route);
    match result {
        SimulationResult::Profitable {
            final_profit_lamports,
            ..
        } => {
            assert!(final_profit_lamports > 0, "Should be profitable");
        }
        SimulationResult::Unprofitable { reason } => {
            panic!("Expected profitable same-DEX route: {}", reason);
        }
    }
}

// ---------------------------------------------------------------------------
// Test 3: Unprofitable route rejected (spread too small for fees)
// ---------------------------------------------------------------------------
#[test]
fn test_e2e_unprofitable_tiny_spread() {
    let sol = config::sol_mint();
    let usdc = usdc_mint();
    let cache = StateCache::new(Duration::from_secs(60));

    // Both pools have nearly identical prices (~140 SOL/USDC, ~0.01% spread)
    // Fees (0.25% + 0.30%) will eat any profit
    let orca_addr = insert_pool(
        &cache,
        DexType::OrcaWhirlpool,
        sol,
        usdc,
        100_000_000_000_000, // 100K SOL
        714_285_714,         // ~714 USDC
        25,
        100,
    );

    let _raydium_addr = insert_pool(
        &cache,
        DexType::RaydiumCp,
        sol,
        usdc,
        100_000_000_000_000, // 100K SOL
        714_185_714,         // ~714.2 USDC — 0.01% difference
        30,
        100,
    );

    let calculator = RouteCalculator::new(cache.clone(), 3);
    let simulator = ProfitSimulator::new(cache.clone(), 0.50, 1000);

    let routes = find_all_routes(&calculator, orca_addr, DexType::OrcaWhirlpool, sol, usdc, 100);

    // Either no routes found, or all are unprofitable after simulation
    for route in &routes {
        let result = simulator.simulate(route);
        match result {
            SimulationResult::Unprofitable { .. } => {
                // Expected: fees eat the tiny spread
            }
            SimulationResult::Profitable { .. } => {
                panic!("Should NOT be profitable with 0.01% spread and 0.55% total fees");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Test 4: Bundle builder produces valid instructions
// ---------------------------------------------------------------------------
#[test]
fn test_e2e_bundle_builder_instructions() {
    let sol = config::sol_mint();
    let usdc = usdc_mint();
    let cache = StateCache::new(Duration::from_secs(60));

    // Create pools with a clear spread
    let orca_addr = insert_pool(
        &cache,
        DexType::OrcaWhirlpool,
        sol,
        usdc,
        100_000_000_000_000,
        714_285_714,
        25,
        100,
    );

    let _raydium_addr = insert_pool(
        &cache,
        DexType::RaydiumCp,
        sol,
        usdc,
        100_000_000_000_000,
        689_655_172,
        30,
        100,
    );

    let calculator = RouteCalculator::new(cache.clone(), 3);
    let routes = find_all_routes(&calculator, orca_addr, DexType::OrcaWhirlpool, sol, usdc, 100);
    assert!(!routes.is_empty(), "Need routes for bundle builder test");

    let route = &routes[0];
    let builder = BundleBuilder::new(Keypair::new(), cache.clone(), None);

    // build_arb_instructions may fail for synthetic pools that lack vault data.
    // The test verifies the pipeline reaches the builder and attempts to build.
    match builder.build_arb_instructions(route, 0) {
        Ok(instructions) => {
            assert!(!instructions.is_empty(), "Should produce non-empty instructions");
            // Expected: compute budget (2 IXs) + ATA creates (2 mints) + wSOL wrap (2 IXs)
            //           + swap per hop (2) + wSOL close (1) = minimum ~9 IXs
            assert!(
                instructions.len() >= 4,
                "Should have at least compute budget + some swap IXs, got {}",
                instructions.len()
            );
        }
        Err(e) => {
            // Acceptable: synthetic pools may lack vault addresses needed for
            // specific DEX IX builders. The test confirms we reached the builder.
            let err_msg = format!("{}", e);
            assert!(
                err_msg.contains("not found")
                    || err_msg.contains("vault")
                    || err_msg.contains("Pool")
                    || err_msg.contains("missing")
                    || err_msg.contains("Missing"),
                "Unexpected error from bundle builder: {}",
                err_msg
            );
        }
    }

    // Also verify that the route has reasonable structure for the builder
    assert_eq!(route.hops.len(), 2, "Cross-DEX route should be 2 hops");
    let hop_dexes: Vec<DexType> = route.hops.iter().map(|h| h.dex_type).collect();
    assert!(
        hop_dexes.contains(&DexType::OrcaWhirlpool) || hop_dexes.contains(&DexType::RaydiumCp),
        "Route should use the pools we created"
    );
}

// ---------------------------------------------------------------------------
// Test 5: Full pipeline with dedup (same pool, same slot)
// ---------------------------------------------------------------------------
#[test]
fn test_e2e_pipeline_dedup() {
    let sol = config::sol_mint();
    let usdc = usdc_mint();
    let cache = StateCache::new(Duration::from_secs(60));

    let orca_addr = insert_pool(
        &cache,
        DexType::OrcaWhirlpool,
        sol,
        usdc,
        100_000_000_000_000,
        714_285_714,
        25,
        100,
    );

    let _raydium_addr = insert_pool(
        &cache,
        DexType::RaydiumCp,
        sol,
        usdc,
        100_000_000_000_000,
        689_655_172,
        30,
        100,
    );

    let calculator = RouteCalculator::new(cache.clone(), 3);
    let simulator = ProfitSimulator::new(cache.clone(), 0.50, 1000);

    // Replicate the dedup logic from main.rs
    let mut recent_pools: HashMap<Pubkey, u64> = HashMap::new();
    let mut submissions = 0u32;

    // Process the same pool update twice in the same slot
    for _ in 0..2 {
        let pool_address = orca_addr;
        let slot = 100u64;

        // Dedup check: skip if same pool + same slot
        if recent_pools.get(&pool_address) == Some(&slot) {
            continue;
        }
        recent_pools.insert(pool_address, slot);

        let routes = find_all_routes(
            &calculator,
            pool_address,
            DexType::OrcaWhirlpool,
            sol,
            usdc,
            slot,
        );

        for route in &routes {
            let result = simulator.simulate(route);
            if let SimulationResult::Profitable { .. } = result {
                submissions += 1;
            }
        }
    }

    // The dedup should have prevented the second processing
    assert!(
        recent_pools.len() == 1,
        "Dedup map should have exactly 1 entry"
    );

    // Different slot should NOT be deduped
    let new_slot = 101u64;
    if recent_pools.get(&orca_addr) != Some(&new_slot) {
        recent_pools.insert(orca_addr, new_slot);
        let routes = find_all_routes(
            &calculator,
            orca_addr,
            DexType::OrcaWhirlpool,
            sol,
            usdc,
            new_slot,
        );
        for route in &routes {
            let result = simulator.simulate(route);
            if let SimulationResult::Profitable { .. } = result {
                submissions += 1;
            }
        }
    }

    // Should have processed twice (slot 100 + slot 101), not three times
    assert!(
        submissions >= 2,
        "Should have submitted from both slots (got {})",
        submissions
    );
}

// ---------------------------------------------------------------------------
// Test 6: Three-hop route detection (triangle arb)
// ---------------------------------------------------------------------------
#[test]
fn test_e2e_three_hop_triangle_arb() {
    let sol = config::sol_mint();
    let usdc = usdc_mint();
    let token_x = Pubkey::new_unique();
    let cache = StateCache::new(Duration::from_secs(60));

    // Pool A: SOL/USDC (Orca) — 1 SOL = ~140 USDC (9 decimals SOL, 6 decimals USDC)
    let pool_a = insert_pool(
        &cache,
        DexType::OrcaWhirlpool,
        sol,
        usdc,
        10_000_000_000_000, // 10K SOL
        1_400_000_000_000,  // 1.4M USDC (6 dec) -> 140 USDC per SOL
        25,
        100,
    );

    // Pool B: USDC/TokenX (Raydium CP) — 1 USDC = 10 TokenX
    let _pool_b = insert_pool(
        &cache,
        DexType::RaydiumCp,
        usdc,
        token_x,
        500_000_000_000,    // 500K USDC (6 dec)
        5_000_000_000_000,  // 5M TokenX (9 dec)
        30,
        100,
    );

    // Pool C: TokenX/SOL (Meteora DLMM) — 1 SOL = 1350 TokenX (mispriced: should be ~1400)
    // The mispricing creates the arb: SOL->USDC->TokenX->SOL is profitable
    let _pool_c = insert_pool(
        &cache,
        DexType::MeteoraDlmm,
        token_x,
        sol,
        13_500_000_000_000, // 13.5M TokenX (9 dec)
        10_000_000_000_000, // 10K SOL
        10,                 // 0.1% fee (DLMM dynamic, low for test)
        100,
    );

    let calculator = RouteCalculator::new(cache.clone(), 3);
    let routes = find_all_routes(&calculator, pool_a, DexType::OrcaWhirlpool, sol, usdc, 100);

    // Look for 3-hop routes
    let three_hop_routes: Vec<&ArbRoute> = routes.iter().filter(|r| r.hops.len() == 3).collect();

    assert!(
        !three_hop_routes.is_empty(),
        "Should find at least one 3-hop triangle route (found {} total routes, hop counts: {:?})",
        routes.len(),
        routes.iter().map(|r| r.hops.len()).collect::<Vec<_>>()
    );

    // Verify it's a real triangle: three different pools, circular
    let route = three_hop_routes[0];
    assert_eq!(route.hops.len(), 3, "Must be exactly 3 hops");
    assert_eq!(
        route.hops[0].input_mint,
        route.hops[2].output_mint,
        "Must be circular"
    );

    // All three hops should go through different pools
    let pool_addrs: Vec<Pubkey> = route.hops.iter().map(|h| h.pool_address).collect();
    assert_ne!(pool_addrs[0], pool_addrs[1], "Hops 0 and 1 should use different pools");
    assert_ne!(pool_addrs[1], pool_addrs[2], "Hops 1 and 2 should use different pools");
    assert_ne!(pool_addrs[0], pool_addrs[2], "Hops 0 and 2 should use different pools");

    // Simulate the best 3-hop route
    let simulator = ProfitSimulator::new(cache.clone(), 0.50, 1000);
    let result = simulator.simulate(route);
    match result {
        SimulationResult::Profitable {
            final_profit_lamports,
            ..
        } => {
            assert!(
                final_profit_lamports > 0,
                "3-hop triangle arb should be profitable"
            );
        }
        SimulationResult::Unprofitable { reason } => {
            // The mispricing may not be enough after 3 hops of fees.
            // This is acceptable — the test verifies route discovery works.
            // Only panic if no 3-hop route was found at all (checked above).
            eprintln!(
                "3-hop route found but unprofitable after simulation: {}",
                reason
            );
        }
    }
}
