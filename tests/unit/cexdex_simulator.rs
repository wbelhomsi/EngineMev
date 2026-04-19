use solana_mev_bot::addresses;
use solana_mev_bot::feed::PriceStore;
use solana_mev_bot::cexdex::route::{ArbDirection, CexDexRoute};
use solana_mev_bot::cexdex::simulator::{CexDexSimulator, CexDexSimulatorConfig, SimulationResult};
use solana_mev_bot::feed::PriceSnapshot;
use solana_mev_bot::router::pool::{DexType, PoolExtra, PoolState};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::time::Instant;

fn usdc_mint() -> Pubkey {
    Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap()
}

fn insert_cp_pool(
    store: &PriceStore,
    sol_reserve: u64,
    usdc_reserve: u64,
    fee_bps: u64,
) -> Pubkey {
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
        extra: PoolExtra::default(),
        best_bid_price: None,
        best_ask_price: None,
    });
    addr
}

fn mk_route_buy(pool: Pubkey, input_usdc_atoms: u64) -> CexDexRoute {
    CexDexRoute {
        pool_address: pool,
        dex_type: DexType::RaydiumCp,
        direction: ArbDirection::BuyOnDex,
        input_mint: usdc_mint(),
        output_mint: addresses::WSOL,
        input_amount: input_usdc_atoms,
        expected_output: 0,
        cex_bid_at_detection: 185.0,
        cex_ask_at_detection: 185.02,
        expected_profit_usd: 1.0,
        observed_slot: 100,
    }
}

fn mk_config() -> CexDexSimulatorConfig {
    CexDexSimulatorConfig {
        min_profit_usd: 0.10,
        slippage_tolerance: 0.25,
        tx_fee_lamports: 5_000,
        min_tip_lamports: 1_000,
        max_tip_fraction: 0.50,
    }
}

#[test]
fn test_profitable_route_passes() {
    let store = PriceStore::new();
    let pool = insert_cp_pool(&store, 100_000_000_000, 18_000_000_000, 30);
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 185.0,
        best_ask_usd: 185.02,
        received_at: Instant::now(),
    });
    let route = mk_route_buy(pool, 100_000_000);

    let sim = CexDexSimulator::new(store, mk_config());
    let result = sim.simulate(&route);
    match result {
        SimulationResult::Profitable {
            net_profit_usd_worst_case,
            adjusted_profit_sol: _,
            adjusted_profit_usd: _,
            min_final_output,
            ..
        } => {
            assert!(net_profit_usd_worst_case > 0.10);
            assert!(min_final_output > 0);
        }
        SimulationResult::Unprofitable { reason } => {
            panic!("expected profitable, got: {}", reason);
        }
    }
}

#[test]
fn test_unprofitable_when_pool_not_cached() {
    let store = PriceStore::new();
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 185.0,
        best_ask_usd: 185.02,
        received_at: Instant::now(),
    });
    let route = mk_route_buy(Pubkey::new_unique(), 100_000_000);

    let sim = CexDexSimulator::new(store, mk_config());
    match sim.simulate(&route) {
        SimulationResult::Unprofitable { reason } => {
            assert!(reason.contains("not found") || reason.contains("cache"));
        }
        _ => panic!("expected unprofitable"),
    }
}

#[test]
fn test_unprofitable_when_profit_below_threshold() {
    let store = PriceStore::new();
    // DEX and CEX nearly identical → tiny profit, below min
    let pool = insert_cp_pool(&store, 100_000_000_000, 18_499_000_000, 30);
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 185.0,
        best_ask_usd: 185.02,
        received_at: Instant::now(),
    });
    let route = mk_route_buy(pool, 10_000_000);

    let sim = CexDexSimulator::new(store, mk_config());
    match sim.simulate(&route) {
        SimulationResult::Unprofitable { .. } => {}
        _ => panic!("expected unprofitable"),
    }
}

/// Hard floor: even if min_profit_usd is misconfigured to 0, the simulator MUST
/// reject any route whose net profit is non-positive after tip + fee.
#[test]
fn test_hard_floor_rejects_non_positive_net_even_with_zero_threshold() {
    let store = PriceStore::new();
    // Tiny spread: gross profit will be minuscule, min_tip floor (1000 lamports)
    // plus tx_fee (5000 lamports) will push net to zero or below.
    let pool = insert_cp_pool(&store, 100_000_000_000, 18_500_900_000, 30);
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 185.0,
        best_ask_usd: 185.02,
        received_at: Instant::now(),
    });
    let route = mk_route_buy(pool, 1_000_000);

    // Deliberately set min_profit_usd = 0.0 to simulate misconfig.
    let mut cfg = mk_config();
    cfg.min_profit_usd = 0.0;
    let sim = CexDexSimulator::new(store, cfg);

    match sim.simulate(&route) {
        SimulationResult::Unprofitable { reason } => {
            assert!(
                reason.contains("non-positive") || reason.contains("not profitable"),
                "expected hard-floor rejection, got: {}", reason,
            );
        }
        SimulationResult::Profitable { net_profit_usd_worst_case, .. } => {
            assert!(
                net_profit_usd_worst_case > 0.0,
                "CRITICAL: simulator approved non-positive worst-case net profit: {}",
                net_profit_usd_worst_case,
            );
        }
    }
}

#[test]
fn profitable_returns_adjusted_profit_sol_and_passes_worst_case_gate() {
    let store = PriceStore::new();
    let pool = insert_cp_pool(&store, 100_000_000_000, 18_000_000_000, 30);
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 185.0,
        best_ask_usd: 185.02,
        received_at: Instant::now(),
    });
    let route = mk_route_buy(pool, 100_000_000);

    // Config with a max tip fraction of 0.40 — sim applies this as worst case.
    let cfg = CexDexSimulatorConfig {
        min_profit_usd: 0.05,
        slippage_tolerance: 0.25,
        tx_fee_lamports: 5_000,
        min_tip_lamports: 1_000,
        max_tip_fraction: 0.40,
    };
    let sim = CexDexSimulator::new(store, cfg);
    match sim.simulate(&route) {
        SimulationResult::Profitable {
            adjusted_profit_sol,
            net_profit_usd_worst_case,
            min_final_output,
            ..
        } => {
            assert!(adjusted_profit_sol > 0.0, "expected positive adjusted profit");
            assert!(net_profit_usd_worst_case >= 0.05, "worst-case net must pass min_profit");
            assert!(min_final_output > 0);
        }
        SimulationResult::Unprofitable { reason } => {
            panic!("expected Profitable; got {}", reason);
        }
    }
}
