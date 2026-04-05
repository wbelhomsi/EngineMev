use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use std::time::Duration;

use solana_mev_bot::router::pool::{ArbRoute, DexType, PoolState, PoolExtra, RouteHop};
use solana_mev_bot::executor::BundleBuilder;
use solana_mev_bot::state::StateCache;

// ─── execute_arb_v2 tests ───────────────────────────────────────────────────

/// Helper: create a cache with Orca + RaydiumCp pools for SOL->TOKEN->SOL
fn setup_multi_dex_cache() -> (StateCache, Pubkey, Pubkey, Pubkey) {
    let cache = StateCache::new(Duration::from_secs(60));
    let sol = solana_mev_bot::config::sol_mint();
    let token = Pubkey::new_unique();
    let token_program = Pubkey::new_from_array([
        6, 221, 246, 225, 215, 101, 161, 147, 217, 203, 225, 70, 206, 235, 121, 172,
        28, 180, 133, 237, 95, 91, 55, 145, 58, 140, 245, 133, 126, 255, 0, 169,
    ]); // TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA

    // Pool A: Orca Whirlpool (SOL -> TOKEN)
    let pool_a = Pubkey::new_unique();
    cache.upsert(pool_a, PoolState {
        address: pool_a,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: sol,
        token_b_mint: token,
        token_a_reserve: 10_000_000_000_000,
        token_b_reserve: 10_000_000_000_000,
        fee_bps: 25,
        current_tick: Some(0),
        sqrt_price_x64: Some(1u128 << 64),
        liquidity: Some(1_000_000_000),
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            tick_spacing: Some(64),
            observation: Some(Pubkey::new_unique()),
            token_program_a: None,
            token_program_b: None,
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    });

    // Pool B: Raydium CP (TOKEN -> SOL)
    let pool_b = Pubkey::new_unique();
    cache.upsert(pool_b, PoolState {
        address: pool_b,
        dex_type: DexType::RaydiumCp,
        token_a_mint: sol,
        token_b_mint: token,
        token_a_reserve: 10_000_000_000_000,
        token_b_reserve: 10_050_000_000_000,
        fee_bps: 25,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            config: Some(Pubkey::new_unique()),
            token_program_a: Some(token_program),
            token_program_b: Some(token_program),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    });

    (cache, pool_a, pool_b, token)
}

/// With arb-guard configured, build_arb_instructions should produce a single
/// execute_arb_v2 IX (via CPI) for any DEX combination, not just Orca.
#[test]
fn test_execute_arb_v2_multi_dex_produces_single_ix() {
    let (cache, pool_a, pool_b, token) = setup_multi_dex_cache();
    let sol = solana_mev_bot::config::sol_mint();
    let guard_id = Pubkey::new_unique();
    let builder = BundleBuilder::new(Keypair::new(), cache, Some(guard_id));

    let route = ArbRoute {
        base_mint: sol,
        input_amount: 1_000_000,
        estimated_profit: 50_000,
        estimated_profit_lamports: 50_000,
        hops: vec![
            RouteHop {
                pool_address: pool_a,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol,
                output_mint: token,
                estimated_output: 1_000_000,
            },
            RouteHop {
                pool_address: pool_b,
                dex_type: DexType::RaydiumCp,
                input_mint: token,
                output_mint: sol,
                estimated_output: 1_050_000,
            },
        ],
    };

    let ixs = builder.build_arb_instructions(&route, 1_000_000).unwrap();

    // Should contain exactly 1 IX targeting the arb-guard program (execute_arb_v2)
    let guard_ixs: Vec<_> = ixs.iter().filter(|ix| ix.program_id == guard_id).collect();
    assert_eq!(guard_ixs.len(), 1, "Should have exactly 1 guard IX (execute_arb_v2), not start_check + profit_check, got {}", guard_ixs.len());
}

/// execute_arb_v2 remaining_accounts should contain accounts from all hops
#[test]
fn test_execute_arb_v2_remaining_accounts_include_all_hops() {
    let (cache, pool_a, pool_b, token) = setup_multi_dex_cache();
    let sol = solana_mev_bot::config::sol_mint();
    let guard_id = Pubkey::new_unique();
    let builder = BundleBuilder::new(Keypair::new(), cache, Some(guard_id));

    let route = ArbRoute {
        base_mint: sol,
        input_amount: 1_000_000,
        estimated_profit: 50_000,
        estimated_profit_lamports: 50_000,
        hops: vec![
            RouteHop {
                pool_address: pool_a,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol,
                output_mint: token,
                estimated_output: 1_000_000,
            },
            RouteHop {
                pool_address: pool_b,
                dex_type: DexType::RaydiumCp,
                input_mint: token,
                output_mint: sol,
                estimated_output: 1_050_000,
            },
        ],
    };

    let ixs = builder.build_arb_instructions(&route, 1_000_000).unwrap();
    let arb_ix = ixs.iter().find(|ix| ix.program_id == guard_id).unwrap();

    // remaining_accounts should include signer + program_ids + pool accounts from both hops
    // Orca swap_v2 has 15 accounts, Raydium CP has 13 accounts, plus signer + 2 program_ids
    // Some accounts may overlap (signer appears in both)
    assert!(arb_ix.accounts.len() >= 10,
        "execute_arb_v2 should have many remaining_accounts, got {}",
        arb_ix.accounts.len());

    // First remaining account must be the signer
    assert!(arb_ix.accounts[0].is_signer,
        "First remaining account must be the signer");
}

/// Without arb-guard configured, build_arb_instructions should still work
/// (falls back to separate swap IXs without guard wrapping)
#[test]
fn test_no_guard_still_builds_swap_ixs() {
    let (cache, pool_a, pool_b, token) = setup_multi_dex_cache();
    let sol = solana_mev_bot::config::sol_mint();
    let builder = BundleBuilder::new(Keypair::new(), cache, None); // no guard

    let route = ArbRoute {
        base_mint: sol,
        input_amount: 1_000_000,
        estimated_profit: 50_000,
        estimated_profit_lamports: 50_000,
        hops: vec![
            RouteHop {
                pool_address: pool_a,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol,
                output_mint: token,
                estimated_output: 1_000_000,
            },
            RouteHop {
                pool_address: pool_b,
                dex_type: DexType::RaydiumCp,
                input_mint: token,
                output_mint: sol,
                estimated_output: 1_050_000,
            },
        ],
    };

    let ixs = builder.build_arb_instructions(&route, 1_000_000).unwrap();

    // No guard program IXs
    let guard_ixs: Vec<_> = ixs.iter().filter(|ix| {
        // Exclude known program IDs (compute budget, ATA, SPL, system, DEX programs)
        let known_programs = [
            solana_mev_bot::addresses::COMPUTE_BUDGET,
            solana_mev_bot::addresses::ATA_PROGRAM,
            solana_mev_bot::addresses::SPL_TOKEN,
            solana_mev_bot::addresses::ORCA_WHIRLPOOL,
            solana_mev_bot::addresses::RAYDIUM_CP,
        ];
        !known_programs.contains(&ix.program_id)
            && ix.program_id != solana_system_interface::program::id()
    }).collect();
    assert!(guard_ixs.is_empty(), "No guard IXs should be present when guard is None, found {} unexpected IXs", guard_ixs.len());

    // Should have compute budget + ATAs + wrap + 2 swaps + unwrap
    assert!(ixs.len() >= 4, "Should have compute budget + ATA + wrap + swaps + unwrap, got {}", ixs.len());
}
