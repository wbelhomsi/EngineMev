use solana_mev_bot::addresses;
use solana_mev_bot::cexdex::detector::{Detector, DetectorConfig};
use solana_mev_bot::cexdex::route::ArbDirection;
use solana_mev_bot::cexdex::{Inventory, PriceStore};
use solana_mev_bot::feed::PriceSnapshot;
use solana_mev_bot::router::pool::{DexType, PoolExtra, PoolState};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::time::Instant;

fn usdc_mint() -> Pubkey {
    Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap()
}

fn mk_detector_config() -> DetectorConfig {
    DetectorConfig {
        min_spread_bps: 15,
        min_profit_usd: 0.10,
        max_trade_size_sol: 10.0,
        max_position_fraction: 1.0, // tests built pre-fraction-cap; allow full-inventory trades
        cex_staleness_ms: 500,
        slippage_tolerance: 0.25,
        dedup_window_ms: 0,           // off by default in tests; explicit tests override
        global_submit_cooldown_ms: 0,
    }
}

/// Build a RaydiumCp pool with given reserves (CPMM for deterministic math).
fn insert_cp_pool(
    store: &PriceStore,
    sol_reserve: u64,
    usdc_reserve: u64,
    fee_bps: u64,
) -> (Pubkey, DexType) {
    let addr = Pubkey::new_unique();
    store.pools.upsert(addr, PoolState {
        address: addr,
        dex_type: DexType::RaydiumCp,
        token_a_mint: addresses::WSOL,
        token_b_mint: usdc_mint(),
        token_a_reserve: sol_reserve,
        token_b_reserve: usdc_reserve,
        fee_bps,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            config: Some(Pubkey::new_unique()),
            token_program_a: Some(addresses::SPL_TOKEN),
            token_program_b: Some(addresses::SPL_TOKEN),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    });
    (addr, DexType::RaydiumCp)
}

#[test]
fn test_no_opportunity_when_prices_aligned() {
    let store = PriceStore::new();
    // Pool at ~185 USDC/SOL: 100 SOL, 18500 USDC → 18500/100 = 185.0
    let (pool_addr, dex) = insert_cp_pool(
        &store,
        100_000_000_000,
        18_500_000_000,
        30,
    );
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 184.99,
        best_ask_usd: 185.01,
        received_at: Instant::now(),
    });

    let inv = Inventory::new_for_test();
    inv.set_on_chain(5_000_000_000, 5_000_000_000);
    inv.set_sol_price_usd(185.0);

    let detector = Detector::new(
        store,
        inv,
        vec![(dex, pool_addr)],
        mk_detector_config(),
    );
    let result = detector.check_all();
    assert!(result.is_none(), "no divergence → no opportunity");
}

#[test]
fn test_detects_buy_on_dex_when_dex_cheap() {
    let store = PriceStore::new();
    // Pool at ~180 USDC/SOL: 100 SOL, 18000 USDC → dex is cheap vs CEX 185
    let (pool_addr, dex) = insert_cp_pool(
        &store,
        100_000_000_000,
        18_000_000_000,
        30,
    );
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 184.99,
        best_ask_usd: 185.01,
        received_at: Instant::now(),
    });

    let inv = Inventory::new_for_test();
    inv.set_on_chain(5_000_000_000, 5_000_000_000);
    inv.set_sol_price_usd(185.0);

    let detector = Detector::new(
        store,
        inv,
        vec![(dex, pool_addr)],
        mk_detector_config(),
    );
    let result = detector.check_all().expect("should find opportunity");
    assert_eq!(result.direction, ArbDirection::BuyOnDex);
    assert!(result.input_amount > 0);
    assert!(result.expected_profit_usd > 0.10,
        "profit {} should exceed min", result.expected_profit_usd);
}

#[test]
fn test_rejects_when_cex_stale() {
    let store = PriceStore::new();
    let (pool_addr, dex) = insert_cp_pool(
        &store,
        100_000_000_000,
        18_000_000_000,
        30,
    );
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 184.99,
        best_ask_usd: 185.01,
        // 2 seconds old, well past the 500ms staleness threshold
        received_at: Instant::now() - std::time::Duration::from_secs(2),
    });

    let inv = Inventory::new_for_test();
    inv.set_on_chain(5_000_000_000, 5_000_000_000);
    inv.set_sol_price_usd(185.0);

    let detector = Detector::new(
        store,
        inv,
        vec![(dex, pool_addr)],
        mk_detector_config(),
    );
    assert!(detector.check_all().is_none(), "stale CEX should reject");
}

#[test]
fn test_rejects_when_inventory_hard_capped() {
    let store = PriceStore::new();
    // Pool at ~180 USDC/SOL: 100 SOL, 18000 USDC — large divergence
    let (pool_addr, dex) = insert_cp_pool(
        &store,
        100_000_000_000,
        18_000_000_000,
        30,
    );
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 184.99,
        best_ask_usd: 185.01,
        received_at: Instant::now(),
    });

    // 9 SOL, 185 USDC → ~90% SOL → hard cap blocks BuyOnDex
    let inv = Inventory::new_for_test();
    inv.set_on_chain(9_000_000_000, 185_000_000);
    inv.set_sol_price_usd(185.0);

    let detector = Detector::new(
        store,
        inv,
        vec![(dex, pool_addr)],
        mk_detector_config(),
    );
    assert!(detector.check_all().is_none(),
        "hard cap should block buy when SOL-heavy");
}

#[test]
fn test_picks_best_opportunity_across_pools() {
    let store = PriceStore::new();
    // pool_a: ~183 USDC/SOL — small divergence
    let (pool_a, _) = insert_cp_pool(&store, 100_000_000_000, 18_300_000_000, 30);
    // pool_b: ~180 USDC/SOL — larger divergence, should be preferred
    let (pool_b, _) = insert_cp_pool(&store, 100_000_000_000, 18_000_000_000, 30);

    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 184.99,
        best_ask_usd: 185.01,
        received_at: Instant::now(),
    });

    let inv = Inventory::new_for_test();
    inv.set_on_chain(5_000_000_000, 100_000_000_000);
    inv.set_sol_price_usd(185.0);

    let detector = Detector::new(
        store,
        inv,
        vec![(DexType::RaydiumCp, pool_a), (DexType::RaydiumCp, pool_b)],
        mk_detector_config(),
    );
    let result = detector.check_all().expect("should find opportunity");
    assert_eq!(result.pool_address, pool_b, "should prefer the more divergent pool");
}

/// Regression: prior behavior sized BuyOnDex at the entire USDC balance when
/// it was below `max_trade_size_sol` (in SOL-equivalent). This led to
/// draining 100% of USDC in a single trade. With `max_position_fraction=0.20`,
/// the trade is capped at 20% of total portfolio (SOL + USDC, SOL-equiv).
#[test]
fn test_position_fraction_cap_limits_buy_on_dex_trade_size() {
    let store = PriceStore::new();
    // Divergent pool: DEX at ~180, CEX at ~185 → BuyOnDex is profitable
    let (pool_addr, _) = insert_cp_pool(&store, 100_000_000_000, 18_000_000_000, 30);
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 184.99,
        best_ask_usd: 185.01,
        received_at: Instant::now(),
    });

    // Wallet: 1 SOL + 100 USDC. Total portfolio in SOL-equiv ≈ 1 + 100/185 = 1.54 SOL.
    // With fraction=0.20 → max trade = 0.308 SOL ≈ $57 USDC.
    let inv = Inventory::new_for_test();
    inv.set_on_chain(1_000_000_000, 100_000_000);
    inv.set_sol_price_usd(185.0);

    let mut cfg = mk_detector_config();
    cfg.max_position_fraction = 0.20;
    cfg.min_profit_usd = 0.01;
    cfg.max_trade_size_sol = 5.0;

    let detector = Detector::new(
        store,
        inv,
        vec![(DexType::RaydiumCp, pool_addr)],
        cfg,
    );
    let result = detector.check_all().expect("should find opportunity");
    assert_eq!(result.direction, ArbDirection::BuyOnDex);

    // Input is in USDC atoms (6 decimals). Must be < 100 USDC (full balance).
    // Expected ~$57 = 57_000_000 atoms, definitely not 100_000_000.
    assert!(
        result.input_amount < 100_000_000,
        "fraction cap should prevent spending 100% of USDC; got {} atoms",
        result.input_amount,
    );
    assert!(
        result.input_amount <= 60_000_000,
        "should cap near 20% of portfolio SOL-equiv (~$57); got {} atoms",
        result.input_amount,
    );
}

/// Regression: detector used to fire the same (pool, direction) on every
/// Geyser tick, producing multiple SUBMITs in a single second. With
/// `dedup_window_ms`, the second call within the window returns None.
#[test]
fn test_dedup_window_blocks_repeated_same_pool_direction() {
    let store = PriceStore::new();
    let (pool_addr, _) = insert_cp_pool(&store, 100_000_000_000, 18_000_000_000, 30);
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 184.99,
        best_ask_usd: 185.01,
        received_at: Instant::now(),
    });
    let inv = Inventory::new_for_test();
    inv.set_on_chain(5_000_000_000, 5_000_000_000);
    inv.set_sol_price_usd(185.0);

    let mut cfg = mk_detector_config();
    cfg.dedup_window_ms = 500;

    let detector = Detector::new(store, inv, vec![(DexType::RaydiumCp, pool_addr)], cfg);

    // First call returns a route (detector emits an opportunity)
    let first = detector.check_all().expect("first call should emit route");
    // Simulate the binary calling mark_dispatched after dispatch
    detector.mark_dispatched(first.pool_address, first.direction);

    // Immediate second call: same (pool, direction) within the dedup window → None
    assert!(
        detector.check_all().is_none(),
        "second call within dedup window should return None",
    );
}

/// Regression: global_submit_cooldown_ms blocks ALL routes (regardless of
/// pool/direction) after any dispatch, protecting against a burst.
#[test]
fn test_global_cooldown_blocks_all_after_dispatch() {
    let store = PriceStore::new();
    let (pool_a, _) = insert_cp_pool(&store, 100_000_000_000, 18_000_000_000, 30);
    let (pool_b, _) = insert_cp_pool(&store, 100_000_000_000, 17_900_000_000, 30);
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 184.99,
        best_ask_usd: 185.01,
        received_at: Instant::now(),
    });
    let inv = Inventory::new_for_test();
    inv.set_on_chain(5_000_000_000, 5_000_000_000);
    inv.set_sol_price_usd(185.0);

    let mut cfg = mk_detector_config();
    cfg.dedup_window_ms = 0; // per-key dedup off; isolate global cooldown behavior
    cfg.global_submit_cooldown_ms = 1_500;

    let detector = Detector::new(
        store,
        inv,
        vec![(DexType::RaydiumCp, pool_a), (DexType::RaydiumCp, pool_b)],
        cfg,
    );

    let first = detector.check_all().expect("first call should emit");
    detector.mark_dispatched(first.pool_address, first.direction);

    // Immediate second call — even on a DIFFERENT pool — should be blocked.
    assert!(
        detector.check_all().is_none(),
        "second call within global cooldown should return None even on different pool",
    );
}
