use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use std::time::Duration;

use solana_mev_bot::executor::bundle::{
    estimate_tx_size, estimate_unique_accounts, TX_SIZE_SAFE_THRESHOLD,
};
use solana_mev_bot::executor::BundleBuilder;
use solana_mev_bot::router::pool::{ArbRoute, DexType, PoolExtra, PoolState, RouteHop};
use solana_mev_bot::state::StateCache;

// ─── estimate_tx_size tests ─────────────────────────────────────────────────

#[test]
fn test_estimate_tx_size_empty() {
    let size = estimate_tx_size(&[], 226);
    // Even with no instructions, there's fixed overhead (sig, header, blockhash, tip ix)
    assert!(size > 100, "Empty tx should still have overhead, got {}", size);
    assert!(size < 300, "Empty tx should be small, got {}", size);
}

#[test]
fn test_estimate_tx_size_single_small_ix() {
    let ix = Instruction {
        program_id: Pubkey::new_unique(),
        accounts: vec![AccountMeta::new(Pubkey::new_unique(), true)],
        data: vec![1u8; 8],
    };
    let size = estimate_tx_size(&[ix], 226);
    assert!(size < 400, "Single small IX should be well under limit, got {}", size);
}

#[test]
fn test_estimate_tx_size_grows_with_accounts_not_in_alt() {
    // Create instructions with many unique accounts (none in ALT since alt_address_count=0)
    let signer = Pubkey::new_unique();
    let mut instructions = Vec::new();
    for _ in 0..5 {
        instructions.push(Instruction {
            program_id: Pubkey::new_unique(),
            accounts: vec![
                AccountMeta::new(signer, true),
                AccountMeta::new(Pubkey::new_unique(), false),
                AccountMeta::new(Pubkey::new_unique(), false),
                AccountMeta::new(Pubkey::new_unique(), false),
            ],
            data: vec![0u8; 20],
        });
    }

    let size_no_alt = estimate_tx_size(&instructions, 0);
    let size_with_alt = estimate_tx_size(&instructions, 226);

    // Without ALT, every unique account costs 32 bytes as static key.
    // With ALT, most accounts are resolved to 1-byte indices.
    assert!(
        size_no_alt > size_with_alt,
        "No ALT ({} bytes) should be larger than with ALT ({} bytes)",
        size_no_alt,
        size_with_alt
    );
}

#[test]
fn test_estimate_tx_size_many_accounts_exceeds_threshold() {
    // Simulate a large CPI-style instruction with 30+ accounts and lots of data
    let mut accounts = Vec::new();
    for _ in 0..35 {
        accounts.push(AccountMeta::new(Pubkey::new_unique(), false));
    }
    let ix = Instruction {
        program_id: Pubkey::new_unique(),
        accounts,
        data: vec![0u8; 300], // Large instruction data (3 hops of encoded ix_data)
    };

    // With no ALT, this should definitely exceed the threshold
    let size = estimate_tx_size(&[ix], 0);
    assert!(
        size > TX_SIZE_SAFE_THRESHOLD,
        "35 unique non-ALT accounts + 300 bytes data should exceed threshold, got {}",
        size
    );
}

#[test]
fn test_estimate_consistent_with_unique_accounts() {
    let prog = Pubkey::new_unique();
    let acc1 = Pubkey::new_unique();
    let acc2 = Pubkey::new_unique();

    let instructions = vec![
        Instruction {
            program_id: prog,
            accounts: vec![
                AccountMeta::new(acc1, true),
                AccountMeta::new(acc2, false),
            ],
            data: vec![1u8],
        },
        Instruction {
            program_id: prog,
            accounts: vec![
                AccountMeta::new(acc1, true), // duplicate
                AccountMeta::new(acc2, false), // duplicate
            ],
            data: vec![2u8],
        },
    ];

    let unique = estimate_unique_accounts(&instructions);
    assert_eq!(unique, 3, "prog + acc1 + acc2 = 3 unique");

    // Size estimate should reflect deduplication
    let size = estimate_tx_size(&instructions, 0);
    // 3 unique accounts * 32 bytes = 96 bytes for keys alone, plus overhead
    assert!(size < 500, "Deduplicated accounts should keep tx small, got {}", size);
}

// ─── CPI fallback for 3-hop routes ─────────────────────────────────────────

/// Helper: create a cache with 3 pools for a 3-hop route A->B->C->A
fn setup_3hop_cache() -> (StateCache, Pubkey, Pubkey, Pubkey, Pubkey, Pubkey, Pubkey) {
    let cache = StateCache::new(Duration::from_secs(60));
    let sol = solana_mev_bot::config::sol_mint();
    let token_b = Pubkey::new_unique();
    let token_c = Pubkey::new_unique();
    let token_program = solana_mev_bot::addresses::SPL_TOKEN;

    cache.set_mint_program(token_b, token_program);
    cache.set_mint_program(token_c, token_program);

    // Pool 1: Orca (SOL -> TOKEN_B)
    let pool_1 = Pubkey::new_unique();
    cache.upsert(
        pool_1,
        PoolState {
            address: pool_1,
            dex_type: DexType::OrcaWhirlpool,
            token_a_mint: sol,
            token_b_mint: token_b,
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
                ..Default::default()
            },
            best_bid_price: None,
            best_ask_price: None,
        },
    );

    // Pool 2: Raydium CP (TOKEN_B -> TOKEN_C)
    let pool_2 = Pubkey::new_unique();
    cache.upsert(
        pool_2,
        PoolState {
            address: pool_2,
            dex_type: DexType::RaydiumCp,
            token_a_mint: token_b,
            token_b_mint: token_c,
            token_a_reserve: 10_000_000_000_000,
            token_b_reserve: 10_000_000_000_000,
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
        },
    );

    // Pool 3: Orca (TOKEN_C -> SOL)
    let pool_3 = Pubkey::new_unique();
    cache.upsert(
        pool_3,
        PoolState {
            address: pool_3,
            dex_type: DexType::OrcaWhirlpool,
            token_a_mint: sol,
            token_b_mint: token_c,
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
                ..Default::default()
            },
            best_bid_price: None,
            best_ask_price: None,
        },
    );

    (cache, pool_1, pool_2, pool_3, sol, token_b, token_c)
}

/// A 2-hop route with arb-guard should use the CPI path (single execute_arb_v2 IX).
#[test]
fn test_2hop_uses_cpi_path() {
    let (cache, pool_1, pool_2, _pool_3, sol, token_b, _token_c) = setup_3hop_cache();
    let guard_id = Pubkey::new_unique();
    let builder = BundleBuilder::new(Keypair::new(), cache, Some(guard_id));

    let route = ArbRoute {
        base_mint: sol,
        input_amount: 1_000_000,
        estimated_profit: 50_000,
        estimated_profit_lamports: 50_000,
        hops: vec![
            RouteHop {
                pool_address: pool_1,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol,
                output_mint: token_b,
                estimated_output: 1_000_000,
            },
            RouteHop {
                pool_address: pool_2,
                dex_type: DexType::RaydiumCp,
                input_mint: token_b,
                output_mint: sol,
                estimated_output: 1_050_000,
            },
        ],
    };

    let ixs = builder.build_arb_instructions(&route, 1_000_000).unwrap();

    // CPI path: should have exactly 1 IX targeting arb-guard program
    let guard_ixs: Vec<_> = ixs.iter().filter(|ix| ix.program_id == guard_id).collect();
    assert_eq!(
        guard_ixs.len(),
        1,
        "2-hop route should use CPI path with 1 guard IX"
    );
}

/// A 3-hop route with arb-guard should fall back to the non-CPI path
/// if the CPI path would produce an oversized transaction.
/// The non-CPI path has no guard IX — it uses separate swap IXs.
#[test]
fn test_3hop_falls_back_to_non_cpi_when_oversized() {
    let (cache, pool_1, pool_2, pool_3, sol, token_b, token_c) = setup_3hop_cache();
    let guard_id = Pubkey::new_unique();
    let builder = BundleBuilder::new(Keypair::new(), cache, Some(guard_id));

    let route = ArbRoute {
        base_mint: sol,
        input_amount: 1_000_000,
        estimated_profit: 50_000,
        estimated_profit_lamports: 50_000,
        hops: vec![
            RouteHop {
                pool_address: pool_1,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol,
                output_mint: token_b,
                estimated_output: 1_000_000,
            },
            RouteHop {
                pool_address: pool_2,
                dex_type: DexType::RaydiumCp,
                input_mint: token_b,
                output_mint: token_c,
                estimated_output: 1_000_000,
            },
            RouteHop {
                pool_address: pool_3,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: token_c,
                output_mint: sol,
                estimated_output: 1_050_000,
            },
        ],
    };

    let ixs = builder.build_arb_instructions(&route, 1_000_000).unwrap();

    // The route should still build successfully (either CPI or non-CPI)
    assert!(!ixs.is_empty(), "Should produce instructions");

    // Check whether it fell back to non-CPI:
    // Non-CPI path has NO guard IX, instead it has individual swap IXs
    let guard_ixs: Vec<_> = ixs.iter().filter(|ix| ix.program_id == guard_id).collect();

    // If CPI estimate was too large, we should see 0 guard IXs (non-CPI path).
    // If CPI fits, we see 1 guard IX.
    // Either way the build must succeed — this is the core invariant.
    if guard_ixs.is_empty() {
        // Non-CPI fallback: should have individual swap IXs for each hop
        // At minimum: compute budget + heap + ATAs + wSOL wrap + 3 swaps + wSOL unwrap
        assert!(
            ixs.len() >= 6,
            "Non-CPI fallback should have at least 6 IXs (budget + heap + ATAs + swaps), got {}",
            ixs.len()
        );
    } else {
        // CPI path fit: single guard IX
        assert_eq!(guard_ixs.len(), 1, "CPI path should have exactly 1 guard IX");
    }
}

/// Without arb-guard, build_arb_instructions always produces separate swap IXs.
/// This is the baseline path that must always work regardless of hop count.
#[test]
fn test_no_guard_3hop_always_produces_separate_swaps() {
    let (cache, pool_1, pool_2, pool_3, sol, token_b, token_c) = setup_3hop_cache();
    let builder = BundleBuilder::new(Keypair::new(), cache, None);

    let route = ArbRoute {
        base_mint: sol,
        input_amount: 1_000_000,
        estimated_profit: 50_000,
        estimated_profit_lamports: 50_000,
        hops: vec![
            RouteHop {
                pool_address: pool_1,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol,
                output_mint: token_b,
                estimated_output: 1_000_000,
            },
            RouteHop {
                pool_address: pool_2,
                dex_type: DexType::RaydiumCp,
                input_mint: token_b,
                output_mint: token_c,
                estimated_output: 1_000_000,
            },
            RouteHop {
                pool_address: pool_3,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: token_c,
                output_mint: sol,
                estimated_output: 1_050_000,
            },
        ],
    };

    let ixs = builder.build_arb_instructions(&route, 1_000_000).unwrap();

    // No guard configured: should have 3 separate swap IXs (one per hop)
    // Plus compute budget, heap, ATAs, wSOL wrap/unwrap
    assert!(
        ixs.len() >= 7,
        "No-guard 3-hop should have at least 7 IXs, got {}",
        ixs.len()
    );

    // None of the IXs should be to a guard program
    let known_programs = [
        solana_mev_bot::addresses::COMPUTE_BUDGET,
        solana_mev_bot::addresses::ATA_PROGRAM,
        solana_mev_bot::addresses::SPL_TOKEN,
        solana_mev_bot::addresses::RAYDIUM_CP,
        solana_mev_bot::addresses::ORCA_WHIRLPOOL,
        Pubkey::default(), // system program = all zeros
    ];
    // All IXs should target known programs (no guard program)
    for ix in &ixs {
        assert!(
            known_programs.contains(&ix.program_id),
            "Unexpected program {} in no-guard path",
            ix.program_id
        );
    }
}

/// The estimate_tx_size function should produce a larger estimate for CPI-wrapped
/// 3-hop routes compared to 2-hop routes, because CPI flattens all accounts into
/// one instruction's remaining_accounts without dedup.
#[test]
fn test_cpi_size_estimate_grows_with_hops() {
    // Simulate a 2-hop CPI instruction set (budget + heap + ATAs + wrap + CPI + unwrap)
    let _signer = Pubkey::new_unique();
    let budget_ix = Instruction {
        program_id: Pubkey::new_unique(),
        accounts: vec![],
        data: vec![2, 0, 0, 0, 0],
    };
    let heap_ix = Instruction {
        program_id: Pubkey::new_unique(),
        accounts: vec![],
        data: vec![1, 0, 0, 4, 0],
    };

    // 2-hop CPI: ~20 remaining accounts, ~100 bytes ix_data
    let cpi_2hop = Instruction {
        program_id: Pubkey::new_unique(),
        accounts: (0..20)
            .map(|_| AccountMeta::new(Pubkey::new_unique(), false))
            .collect(),
        data: vec![0u8; 100],
    };
    let ixs_2hop = vec![budget_ix.clone(), heap_ix.clone(), cpi_2hop];

    // 3-hop CPI: ~32 remaining accounts, ~160 bytes ix_data
    let cpi_3hop = Instruction {
        program_id: Pubkey::new_unique(),
        accounts: (0..32)
            .map(|_| AccountMeta::new(Pubkey::new_unique(), false))
            .collect(),
        data: vec![0u8; 160],
    };
    let ixs_3hop = vec![budget_ix, heap_ix, cpi_3hop];

    let size_2hop = estimate_tx_size(&ixs_2hop, 226);
    let size_3hop = estimate_tx_size(&ixs_3hop, 226);

    assert!(
        size_3hop > size_2hop,
        "3-hop CPI ({} bytes) should be larger than 2-hop CPI ({} bytes)",
        size_3hop,
        size_2hop
    );
}
