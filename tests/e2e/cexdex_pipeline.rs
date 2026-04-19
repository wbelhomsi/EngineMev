//! E2E test: synthetic Binance price + synthetic pool → detector → simulator → bundle IXs.
//!
//! Run with: cargo test --features e2e --test e2e cexdex_pipeline

use solana_mev_bot::addresses;
use solana_mev_bot::cexdex::detector::{Detector, DetectorConfig};
use solana_mev_bot::cexdex::simulator::{CexDexSimulator, CexDexSimulatorConfig, SimulationResult};
use solana_mev_bot::cexdex::{ArbDirection, Inventory};
use solana_mev_bot::executor::BundleBuilder;
use solana_mev_bot::feed::{PriceSnapshot, PriceStore};
use solana_mev_bot::router::pool::{DexType, PoolExtra, PoolState};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use std::str::FromStr;
use std::time::Instant;

fn usdc() -> Pubkey {
    Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap()
}

/// Insert a SOL/USDC CPMM pool into the PriceStore and return its address.
///
/// `sol_reserve` is in lamports, `usdc_reserve` is in USDC atoms (6 decimals).
fn insert_pool(store: &PriceStore, sol_reserve: u64, usdc_reserve: u64) -> Pubkey {
    let addr = Pubkey::new_unique();
    store.pools.upsert(
        addr,
        PoolState {
            address: addr,
            dex_type: DexType::RaydiumCp,
            token_a_mint: addresses::WSOL,
            token_b_mint: usdc(),
            token_a_reserve: sol_reserve,
            token_b_reserve: usdc_reserve,
            fee_bps: 30,
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
        },
    );
    addr
}

fn detector_config() -> DetectorConfig {
    DetectorConfig {
        min_spread_bps: 15,
        min_profit_usd: 0.10,
        max_trade_size_sol: 5.0,
        max_position_fraction: 1.0,
        cex_staleness_ms: 500,
        slippage_tolerance: 0.25,
        dedup_window_ms: 0,
        global_submit_cooldown_ms: 0,
    }
}

fn sim_config() -> CexDexSimulatorConfig {
    CexDexSimulatorConfig {
        min_profit_usd: 0.10,
        slippage_tolerance: 0.25,
        tx_fee_lamports: 5_000,
        min_tip_lamports: 1_000,
        max_tip_fraction: 0.50,
    }
}

/// Pre-populate mint → token program mapping so BundleBuilder can resolve
/// token programs without hitting the RPC.
fn pre_cache_mint_programs(store: &PriceStore) {
    store.pools.set_mint_program(addresses::WSOL, addresses::SPL_TOKEN);
    store.pools.set_mint_program(usdc(), addresses::SPL_TOKEN);
}

// ---------------------------------------------------------------------------
// Test 1: Happy path — BuyOnDex detected, simulated, bundle IXs built.
// DEX price is cheaper than CEX → we buy SOL on-chain with USDC.
// ---------------------------------------------------------------------------
#[test]
fn test_cex_dex_full_pipeline_buy_on_dex() {
    let store = PriceStore::new();
    // DEX: 100K SOL : 18M USDC → ~$180/SOL (cheap vs CEX $185)
    let pool_addr = insert_pool(&store, 100_000_000_000_000, 18_000_000_000_000);
    // CEX: ~$185/SOL
    store.update_cex(
        "SOLUSDC",
        PriceSnapshot { best_bid_usd: 185.0, best_ask_usd: 185.02, received_at: Instant::now() },
    );
    pre_cache_mint_programs(&store);

    let inv = Inventory::new_for_test();
    // 2 SOL + 2000 USDC on-chain
    inv.set_on_chain(2_000_000_000, 2_000_000_000);
    inv.set_sol_price_usd(185.0);

    let detector = Detector::new(
        store.clone(),
        inv,
        vec![(DexType::RaydiumCp, pool_addr)],
        detector_config(),
    );

    let route = detector.check_all().expect("should detect BuyOnDex opportunity");
    assert_eq!(route.direction, ArbDirection::BuyOnDex, "expected BuyOnDex direction");

    let simulator = CexDexSimulator::new(store.clone(), sim_config());
    let (route, min_final_output) = match simulator.simulate(&route) {
        SimulationResult::Profitable { route, min_final_output, .. } => (route, min_final_output),
        SimulationResult::Unprofitable { reason } => panic!("expected profitable: {}", reason),
    };
    assert!(min_final_output > 0, "min_final_output must be non-zero");

    let signer = Keypair::new();
    let builder = BundleBuilder::new(signer.insecure_clone(), store.pools.clone(), Some(Pubkey::new_unique()));

    let instructions =
        solana_mev_bot::cexdex::bundle::build_instructions_for_cex_dex(&builder, &route, min_final_output)
            .expect("bundle build should succeed");

    assert!(
        instructions.len() >= 4,
        "expected compute budget + ATA creates + swap, got {}",
        instructions.len(),
    );
}

// ---------------------------------------------------------------------------
// Test 2: SellOnDex — DEX price is expensive vs CEX; we sell SOL on-chain.
// ---------------------------------------------------------------------------
#[test]
fn test_cex_dex_pipeline_sell_on_dex() {
    let store = PriceStore::new();
    // DEX: 100K SOL : 19M USDC → ~$190/SOL (expensive vs CEX $185)
    let pool_addr = insert_pool(&store, 100_000_000_000_000, 19_000_000_000_000);
    // CEX: ~$185/SOL
    store.update_cex(
        "SOLUSDC",
        PriceSnapshot { best_bid_usd: 185.0, best_ask_usd: 185.02, received_at: Instant::now() },
    );
    pre_cache_mint_programs(&store);

    let inv = Inventory::new_for_test();
    // 10 SOL + 2000 USDC on-chain (plenty of SOL to sell)
    inv.set_on_chain(10_000_000_000, 2_000_000_000);
    inv.set_sol_price_usd(185.0);

    let detector = Detector::new(
        store.clone(),
        inv,
        vec![(DexType::RaydiumCp, pool_addr)],
        detector_config(),
    );

    let route = detector.check_all().expect("should detect SellOnDex opportunity");
    assert_eq!(route.direction, ArbDirection::SellOnDex, "expected SellOnDex direction");

    let simulator = CexDexSimulator::new(store, sim_config());
    match simulator.simulate(&route) {
        SimulationResult::Profitable { net_profit_usd_worst_case, adjusted_profit_sol, .. } => {
            assert!(net_profit_usd_worst_case > 0.0, "worst-case net profit must be positive");
            assert!(adjusted_profit_sol > 0.0, "adjusted profit in SOL must be positive");
        }
        SimulationResult::Unprofitable { reason } => panic!("expected profitable: {}", reason),
    }
}

// ---------------------------------------------------------------------------
// Test 3: Unprofitable stale state — pool moves to aligned price after
// detection; simulator must reject.
// ---------------------------------------------------------------------------
#[test]
fn test_cex_dex_pipeline_rejects_when_state_moves() {
    let store = PriceStore::new();
    // Pool starts cheap: ~$180/SOL
    let pool_addr = insert_pool(&store, 100_000_000_000_000, 18_000_000_000_000);
    store.update_cex(
        "SOLUSDC",
        PriceSnapshot { best_bid_usd: 185.0, best_ask_usd: 185.02, received_at: Instant::now() },
    );
    pre_cache_mint_programs(&store);

    let inv = Inventory::new_for_test();
    inv.set_on_chain(2_000_000_000, 2_000_000_000);
    inv.set_sol_price_usd(185.0);

    let detector = Detector::new(
        store.clone(),
        inv,
        vec![(DexType::RaydiumCp, pool_addr)],
        detector_config(),
    );
    let route = detector.check_all().expect("should detect divergence on cheap pool");

    // Simulate state movement: pool is now aligned with CEX ($185/SOL).
    // Upsert the same address with updated reserves.
    store.pools.upsert(
        pool_addr,
        PoolState {
            address: pool_addr,
            dex_type: DexType::RaydiumCp,
            token_a_mint: addresses::WSOL,
            token_b_mint: usdc(),
            token_a_reserve: 100_000_000_000_000,
            token_b_reserve: 18_500_000_000_000, // aligned: ~$185/SOL
            fee_bps: 30,
            current_tick: None,
            sqrt_price_x64: None,
            liquidity: None,
            last_slot: 101, // newer slot
            extra: PoolExtra::default(),
            best_bid_price: None,
            best_ask_price: None,
        },
    );

    let simulator = CexDexSimulator::new(store, sim_config());
    match simulator.simulate(&route) {
        SimulationResult::Unprofitable { .. } => {
            // Correct: stale state is detected and rejected.
        }
        SimulationResult::Profitable { net_profit_usd_worst_case, .. } => {
            panic!("stale state should have been rejected, got net_profit_usd_worst_case={net_profit_usd_worst_case:.4}");
        }
    }
}
