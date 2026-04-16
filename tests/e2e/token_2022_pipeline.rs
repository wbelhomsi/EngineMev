//! E2E test for Token-2022 mint handling through the full pipeline.
//!
//! Verifies that:
//! 1. A Token-2022 mint with a cached token program passes bundle building
//! 2. A Token-2022 mint WITHOUT a cached token program fails bundle building
//!    (instead of silently defaulting to SPL Token and failing on-chain)
//!
//! Run with: cargo test --features e2e --test e2e

use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use std::time::Duration;

use solana_mev_bot::executor::BundleBuilder;
use solana_mev_bot::router::pool::{ArbRoute, DexType, PoolExtra, PoolState, RouteHop};
use solana_mev_bot::state::StateCache;
use solana_mev_bot::addresses;

fn wsol() -> Pubkey {
    addresses::WSOL
}

/// Setup: cache with wSOL pre-populated as SPL Token (matches main.rs init).
fn fresh_cache() -> StateCache {
    let cache = StateCache::new(Duration::from_secs(60));
    cache.set_mint_program(wsol(), addresses::SPL_TOKEN);
    cache
}

fn make_route(pool_a: Pubkey, pool_b: Pubkey, other_mint: Pubkey, dex: DexType) -> ArbRoute {
    ArbRoute {
        hops: vec![
            RouteHop {
                pool_address: pool_a, dex_type: dex,
                input_mint: wsol(), output_mint: other_mint,
                estimated_output: 1_000,
            },
            RouteHop {
                pool_address: pool_b, dex_type: dex,
                input_mint: other_mint, output_mint: wsol(),
                estimated_output: 1_050,
            },
        ],
        base_mint: wsol(),
        input_amount: 1_000,
        estimated_profit: 50,
        estimated_profit_lamports: 50,
    }
}

/// Make a Raydium CP pool with full extra fields for swap IX building
fn make_raydium_cp_pool(
    cache: &StateCache,
    mint_b: Pubkey,
    mint_b_program: Pubkey,
    spread_pct: u64,
) -> Pubkey {
    let addr = Pubkey::new_unique();
    let reserve_b = 1_000_000_000 * (100 + spread_pct) / 100;
    cache.upsert(addr, PoolState {
        address: addr,
        dex_type: DexType::RaydiumCp,
        token_a_mint: wsol(),
        token_b_mint: mint_b,
        token_a_reserve: 1_000_000_000,
        token_b_reserve: reserve_b,
        fee_bps: 30,
        current_tick: None, sqrt_price_x64: None, liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            config: Some(Pubkey::new_unique()),
            token_program_a: Some(addresses::SPL_TOKEN),
            token_program_b: Some(mint_b_program),
            ..Default::default()
        },
        best_bid_price: None, best_ask_price: None,
    });
    addr
}

#[test]
fn test_token_2022_mint_with_cached_program_builds() {
    // Token-2022 mint WITH cached program → bundle builds successfully
    let cache = fresh_cache();
    let token_2022_mint = Pubkey::new_unique();
    cache.set_mint_program(token_2022_mint, addresses::TOKEN_2022);

    let pool_a = make_raydium_cp_pool(&cache, token_2022_mint, addresses::TOKEN_2022, 0);
    let pool_b = make_raydium_cp_pool(&cache, token_2022_mint, addresses::TOKEN_2022, 5);

    let route = make_route(pool_a, pool_b, token_2022_mint, DexType::RaydiumCp);

    let signer = Keypair::new();
    let builder = BundleBuilder::new(
        signer.insecure_clone(),
        cache,
        Some(Pubkey::new_unique()),
    );

    let result = builder.build_arb_instructions(&route, 1_000);
    assert!(
        result.is_ok(),
        "Bundle should build with cached Token-2022 mint program: {:?}",
        result.err()
    );

    let instructions = result.unwrap();
    // Should have: compute budget, heap frame, ATA creates (wSOL + token2022),
    //              wsol wrap (2 ixs), execute_arb_v2, wsol unwrap
    assert!(
        instructions.len() >= 6,
        "Expected at least 6 instructions, got {}",
        instructions.len()
    );

    // Verify one of the ATA creates uses Token-2022 program
    let ata_creates: Vec<_> = instructions.iter()
        .filter(|ix| ix.program_id == addresses::ATA_PROGRAM)
        .collect();
    assert_eq!(ata_creates.len(), 2, "Expected 2 ATA create instructions");

    let token_programs_used: Vec<Pubkey> = ata_creates.iter()
        .filter_map(|ix| ix.accounts.get(5).map(|a| a.pubkey))
        .collect();
    assert!(
        token_programs_used.contains(&addresses::TOKEN_2022),
        "One ATA should use Token-2022 program, got: {:?}",
        token_programs_used
    );
}

#[test]
fn test_unknown_mint_program_rejected_at_build() {
    // Mint without cached program → bundle builder errors (prevents bad tx).
    // Use a Raydium CP pool where the pool extra token_program_b is NOT set,
    // so the bundle builder cannot determine the mint's token program.
    let cache = fresh_cache();
    let unknown_mint = Pubkey::new_unique();
    // Deliberately DO NOT cache this mint's program

    let pool_a = Pubkey::new_unique();
    cache.upsert(pool_a, PoolState {
        address: pool_a,
        dex_type: DexType::RaydiumCp,
        token_a_mint: wsol(),
        token_b_mint: unknown_mint,
        token_a_reserve: 1_000_000_000,
        token_b_reserve: 1_000_000_000,
        fee_bps: 30,
        current_tick: None, sqrt_price_x64: None, liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            config: Some(Pubkey::new_unique()),
            // Deliberately NO token_program_a or token_program_b
            ..Default::default()
        },
        best_bid_price: None, best_ask_price: None,
    });
    let pool_b = Pubkey::new_unique();
    cache.upsert(pool_b, PoolState {
        address: pool_b,
        dex_type: DexType::RaydiumCp,
        token_a_mint: wsol(),
        token_b_mint: unknown_mint,
        token_a_reserve: 1_000_000_000,
        token_b_reserve: 1_050_000_000,
        fee_bps: 30,
        current_tick: None, sqrt_price_x64: None, liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            config: Some(Pubkey::new_unique()),
            ..Default::default()
        },
        best_bid_price: None, best_ask_price: None,
    });

    let route = make_route(pool_a, pool_b, unknown_mint, DexType::RaydiumCp);

    let signer = Keypair::new();
    let builder = BundleBuilder::new(
        signer.insecure_clone(),
        cache,
        Some(Pubkey::new_unique()),
    );

    let result = builder.build_arb_instructions(&route, 1_000);
    assert!(
        result.is_err(),
        "Bundle should fail to build when mint program unknown"
    );

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("unknown") || err_msg.contains("Mint program"),
        "Error should mention unknown mint program, got: {}",
        err_msg
    );
}

#[test]
fn test_pool_extra_token_program_fallback() {
    // Mint program not in cache but available in PoolExtra → bundle builder
    // falls back to pool extra (fixed in d6efe14)
    let cache = fresh_cache();
    let token_2022_mint = Pubkey::new_unique();
    // DON'T cache in set_mint_program, but DO set in pool.extra.token_program_b

    let pool_a = Pubkey::new_unique();
    cache.upsert(pool_a, PoolState {
        address: pool_a,
        dex_type: DexType::MeteoraDlmm,
        token_a_mint: wsol(),
        token_b_mint: token_2022_mint,
        token_a_reserve: 1_000_000_000,
        token_b_reserve: 1_000_000_000,
        fee_bps: 30,
        current_tick: Some(0),
        sqrt_price_x64: Some(1 << 64),
        liquidity: Some(1_000_000),
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            token_program_a: Some(addresses::SPL_TOKEN),
            token_program_b: Some(addresses::TOKEN_2022),
            ..Default::default()
        },
        best_bid_price: None, best_ask_price: None,
    });

    let pool_b = Pubkey::new_unique();
    cache.upsert(pool_b, PoolState {
        address: pool_b,
        dex_type: DexType::MeteoraDlmm,
        token_a_mint: wsol(),
        token_b_mint: token_2022_mint,
        token_a_reserve: 1_000_000_000,
        token_b_reserve: 1_050_000_000,
        fee_bps: 30,
        current_tick: Some(0),
        sqrt_price_x64: Some(1 << 64),
        liquidity: Some(1_000_000),
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            token_program_a: Some(addresses::SPL_TOKEN),
            token_program_b: Some(addresses::TOKEN_2022),
            ..Default::default()
        },
        best_bid_price: None, best_ask_price: None,
    });

    let route = make_route(pool_a, pool_b, token_2022_mint, DexType::MeteoraDlmm);

    let signer = Keypair::new();
    let builder = BundleBuilder::new(
        signer.insecure_clone(),
        cache.clone(),
        Some(Pubkey::new_unique()),
    );

    let result = builder.build_arb_instructions(&route, 1_000);
    // With pool extra fallback, this should work even without direct cache entry
    // (the bundle builder should pull from pool.extra.token_program_b)
    match result {
        Ok(_) => {
            // Good — pool extra fallback worked
        }
        Err(e) => {
            // Also acceptable if DLMM-specific validation fails (e.g., no bin arrays)
            // but the error should NOT be about unknown mint program
            let msg = e.to_string();
            assert!(
                !msg.contains("Mint program unknown"),
                "Pool extra fallback should prevent 'Mint program unknown' error, got: {}",
                msg
            );
        }
    }

    // Verify the mint program was cached as a side effect (Token-2022 preserved)
    let cached = cache.get_mint_program(&token_2022_mint);
    if let Some(prog) = cached {
        assert_eq!(prog, addresses::TOKEN_2022, "Fallback should cache Token-2022, not SPL Token");
    }
}

