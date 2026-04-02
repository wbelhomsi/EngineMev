use solana_sdk::pubkey::Pubkey;
use std::time::Duration;

use solana_mev_bot::config;
use solana_mev_bot::router::pool::{DexType, DetectedSwap, PoolExtra, PoolState};
use solana_mev_bot::router::RouteCalculator;
use solana_mev_bot::state::StateCache;

fn sol_mint() -> Pubkey {
    config::sol_mint()
}

fn jitosol_mint() -> Pubkey {
    config::lst_mints()[0].0
}

/// Create a Sanctum virtual pool for jitoSOL/SOL at a given rate.
fn sanctum_virtual_pool(rate: f64, address: Pubkey) -> PoolState {
    let reserve_a: u64 = 1_000_000_000_000_000;
    let reserve_b: u64 = (reserve_a as f64 / rate) as u64;
    PoolState {
        address,
        dex_type: DexType::SanctumInfinity,
        token_a_mint: sol_mint(),
        token_b_mint: jitosol_mint(),
        token_a_reserve: reserve_a,
        token_b_reserve: reserve_b,
        fee_bps: 3,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
    }
}

/// Create a DEX pool for jitoSOL/SOL with given reserves.
fn dex_pool(dex_type: DexType, address: Pubkey, sol_reserve: u64, jitosol_reserve: u64) -> PoolState {
    PoolState {
        address,
        dex_type,
        token_a_mint: sol_mint(),
        token_b_mint: jitosol_mint(),
        token_a_reserve: sol_reserve,
        token_b_reserve: jitosol_reserve,
        fee_bps: dex_type.base_fee_bps(),
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(),
    }
}

#[test]
fn test_route_discovery_dex_to_sanctum() {
    // Setup: Orca has jitoSOL/SOL at effective rate 1.075 (cheap jitoSOL)
    // Sanctum has jitoSOL/SOL at rate 1.082 (oracle rate)
    // Expected: SOL -> jitoSOL (Orca, cheap) -> SOL (Sanctum, expensive) = profit
    let cache = StateCache::new(Duration::from_secs(60));

    let orca_addr = Pubkey::new_unique();
    let sanctum_addr = Pubkey::new_unique();

    // Orca pool: 100K SOL, ~95238 jitoSOL (effective rate ~1.050 — cheap jitoSOL)
    // Sanctum rate = 1.082 => ~3% spread. 1% of Orca reserves = 1K SOL auto-input,
    // which is ~1% price impact — still profitable with 3% spread.
    let orca_pool = dex_pool(
        DexType::OrcaWhirlpool,
        orca_addr,
        100_000_000_000_000, // 100K SOL
        95_238_095_238_095,  // ~95238 jitoSOL -> rate ~1.050
    );

    // Sanctum virtual pool at oracle rate 1.082 (huge reserves, negligible impact)
    let sanctum_pool = sanctum_virtual_pool(1.082, sanctum_addr);

    cache.upsert(orca_addr, orca_pool);
    cache.upsert(sanctum_addr, sanctum_pool);

    let calculator = RouteCalculator::new(cache.clone(), 3);

    // Trigger: someone just swapped on the Orca pool.
    // main.rs searches both directions — do the same here.
    let trigger_fwd = DetectedSwap {
        signature: String::new(),
        dex_type: DexType::OrcaWhirlpool,
        pool_address: orca_addr,
        input_mint: sol_mint(),
        output_mint: jitosol_mint(),
        amount: None,
        observed_slot: 100,
    };
    let trigger_rev = DetectedSwap {
        signature: String::new(),
        dex_type: DexType::OrcaWhirlpool,
        pool_address: orca_addr,
        input_mint: jitosol_mint(),
        output_mint: sol_mint(),
        amount: None,
        observed_slot: 100,
    };

    let mut routes = calculator.find_routes(&trigger_fwd);
    routes.extend(calculator.find_routes(&trigger_rev));
    routes.sort_by(|a, b| b.estimated_profit.cmp(&a.estimated_profit));

    assert!(!routes.is_empty(), "Should find at least one LST arb route");
    assert!(routes[0].is_profitable(), "Best route should be profitable");
    assert_eq!(routes[0].hop_count(), 2, "Should be a 2-hop route");
}

#[test]
fn test_no_route_when_no_spread() {
    // Both pools at same rate -> no profitable route
    let cache = StateCache::new(Duration::from_secs(60));

    let orca_addr = Pubkey::new_unique();
    let sanctum_addr = Pubkey::new_unique();

    // Both at rate 1.082
    let orca_pool = dex_pool(
        DexType::OrcaWhirlpool,
        orca_addr,
        10_000_000_000_000,
        (10_000_000_000_000f64 / 1.082) as u64,
    );
    let sanctum_pool = sanctum_virtual_pool(1.082, sanctum_addr);

    cache.upsert(orca_addr, orca_pool);
    cache.upsert(sanctum_addr, sanctum_pool);

    let calculator = RouteCalculator::new(cache.clone(), 3);

    let trigger = DetectedSwap {
        signature: String::new(),
        dex_type: DexType::OrcaWhirlpool,
        pool_address: orca_addr,
        input_mint: sol_mint(),
        output_mint: jitosol_mint(),
        amount: None,
        observed_slot: 100,
    };

    let routes = calculator.find_routes(&trigger);
    let profitable: Vec<_> = routes.iter().filter(|r| r.is_profitable()).collect();
    assert!(profitable.is_empty(), "No profitable route when rates are equal (fees eat any tiny diff)");
}
