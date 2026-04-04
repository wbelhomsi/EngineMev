use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use std::time::Duration;

use solana_mev_bot::router::pool::{ArbRoute, DexType, PoolState, PoolExtra, RouteHop};
use solana_mev_bot::executor::BundleBuilder;
use solana_mev_bot::state::StateCache;

#[test]
fn test_build_execute_arb_ix_single_hop_orca() {
    let keypair = Keypair::new();
    let guard_program = Pubkey::new_unique();
    let state_cache = StateCache::new(Duration::from_secs(60));

    let wsol = Pubkey::new_from_array([
        6, 152, 134, 5, 195, 244, 216, 167, 113, 13, 62, 29, 93, 46, 138, 101,
        136, 30, 64, 184, 35, 197, 230, 140, 109, 76, 62, 68, 196, 28, 154, 140,
    ]); // So111...112
    let usdc = Pubkey::new_unique();
    let pool_address = Pubkey::new_unique();

    let pool = PoolState {
        address: pool_address,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: wsol,
        token_b_mint: usdc,
        token_a_reserve: 1_000_000_000,
        token_b_reserve: 1_000_000_000,
        fee_bps: 30,
        current_tick: Some(0),
        sqrt_price_x64: Some(1u128 << 64),
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            tick_spacing: Some(64),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    };
    state_cache.upsert(pool_address, pool);

    let builder = BundleBuilder::new(keypair, state_cache, Some(guard_program));

    let route = ArbRoute {
        hops: vec![
            RouteHop {
                pool_address,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: wsol,
                output_mint: usdc,
                estimated_output: 150_000,
            },
        ],
        base_mint: wsol,
        input_amount: 1_000_000,
        estimated_profit: 0,
        estimated_profit_lamports: 0,
    };

    let result = builder.build_execute_arb_ix(&route, 100_000);
    assert!(result.is_ok(), "build_execute_arb_ix should succeed: {:?}", result.err());

    let ix = result.unwrap();
    assert_eq!(ix.program_id, guard_program, "IX should target arb-guard program");
    // Fixed accounts (6) + per-hop accounts (9) = 15
    assert_eq!(ix.accounts.len(), 15, "Should have 6 fixed + 9 per-hop accounts");
    assert!(ix.accounts[0].is_signer, "First account should be signer");
}

#[test]
fn test_build_execute_arb_ix_two_hop_orca() {
    let keypair = Keypair::new();
    let guard_program = Pubkey::new_unique();
    let state_cache = StateCache::new(Duration::from_secs(60));

    let wsol = Pubkey::new_from_array([
        6, 152, 134, 5, 195, 244, 216, 167, 113, 13, 62, 29, 93, 46, 138, 101,
        136, 30, 64, 184, 35, 197, 230, 140, 109, 76, 62, 68, 196, 28, 154, 140,
    ]);
    let usdc = Pubkey::new_unique();
    let pool_a_addr = Pubkey::new_unique();
    let pool_b_addr = Pubkey::new_unique();

    for (addr, mint_a, mint_b) in [(pool_a_addr, wsol, usdc), (pool_b_addr, usdc, wsol)] {
        state_cache.upsert(addr, PoolState {
            address: addr,
            dex_type: DexType::OrcaWhirlpool,
            token_a_mint: mint_a,
            token_b_mint: mint_b,
            token_a_reserve: 1_000_000_000,
            token_b_reserve: 1_000_000_000,
            fee_bps: 30,
            current_tick: Some(0),
            sqrt_price_x64: Some(1u128 << 64),
            liquidity: None,
            last_slot: 100,
            extra: PoolExtra {
                vault_a: Some(Pubkey::new_unique()),
                vault_b: Some(Pubkey::new_unique()),
                tick_spacing: Some(64),
                ..Default::default()
            },
            best_bid_price: None,
            best_ask_price: None,
        });
    }

    let builder = BundleBuilder::new(keypair, state_cache, Some(guard_program));

    let route = ArbRoute {
        hops: vec![
            RouteHop {
                pool_address: pool_a_addr,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: wsol,
                output_mint: usdc,
                estimated_output: 150_000,
            },
            RouteHop {
                pool_address: pool_b_addr,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: usdc,
                output_mint: wsol,
                estimated_output: 1_050_000,
            },
        ],
        base_mint: wsol,
        input_amount: 1_000_000,
        estimated_profit: 50_000,
        estimated_profit_lamports: 50_000,
    };

    let result = builder.build_execute_arb_ix(&route, 1_000_000);
    assert!(result.is_ok(), "2-hop build should succeed: {:?}", result.err());

    let ix = result.unwrap();
    // Fixed accounts (6) + hop1 (9) + hop2 (9) = 24
    assert_eq!(ix.accounts.len(), 24, "Should have 6 fixed + 18 per-hop accounts");
}

#[test]
fn test_build_execute_arb_ix_rejects_non_orca() {
    let keypair = Keypair::new();
    let guard_program = Pubkey::new_unique();
    let state_cache = StateCache::new(Duration::from_secs(60));

    let wsol = Pubkey::new_from_array([
        6, 152, 134, 5, 195, 244, 216, 167, 113, 13, 62, 29, 93, 46, 138, 101,
        136, 30, 64, 184, 35, 197, 230, 140, 109, 76, 62, 68, 196, 28, 154, 140,
    ]);

    let builder = BundleBuilder::new(keypair, state_cache, Some(guard_program));

    let route = ArbRoute {
        hops: vec![
            RouteHop {
                pool_address: Pubkey::new_unique(),
                dex_type: DexType::RaydiumAmm, // NOT Orca
                input_mint: wsol,
                output_mint: Pubkey::new_unique(),
                estimated_output: 150_000,
            },
        ],
        base_mint: wsol,
        input_amount: 1_000_000,
        estimated_profit: 0,
        estimated_profit_lamports: 0,
    };

    let result = builder.build_execute_arb_ix(&route, 100_000);
    assert!(result.is_err(), "Should reject non-Orca hops");
}
