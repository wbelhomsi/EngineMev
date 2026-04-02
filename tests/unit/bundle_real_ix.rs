use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use solana_mev_bot::executor::bundle::{build_raydium_cp_swap_ix, build_damm_v2_swap_ix};
use solana_mev_bot::router::pool::{DexType, PoolState, PoolExtra};

/// Helper: build a PoolState with filled PoolExtra for testing.
fn make_test_pool(dex_type: DexType) -> PoolState {
    PoolState {
        address: Pubkey::new_unique(),
        dex_type,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1_000_000_000,
        token_b_reserve: 2_000_000_000,
        fee_bps: 25,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            config: Some(Pubkey::new_unique()),
            token_program_a: Some(Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()),
            token_program_b: Some(Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()),
        },
    }
}

#[test]
fn test_raydium_cp_swap_ix_account_count() {
    let pool = make_test_pool(DexType::RaydiumCp);
    let signer = Pubkey::new_unique();
    let ix = build_raydium_cp_swap_ix(
        &signer, &pool, pool.token_a_mint, 1_000_000, 900_000,
    );
    assert!(ix.is_some(), "Should produce an instruction with full PoolExtra");
    let ix = ix.unwrap();
    assert_eq!(ix.accounts.len(), 13, "Raydium CP swap requires 13 accounts");
}

#[test]
fn test_raydium_cp_swap_ix_discriminator() {
    let pool = make_test_pool(DexType::RaydiumCp);
    let signer = Pubkey::new_unique();
    let ix = build_raydium_cp_swap_ix(
        &signer, &pool, pool.token_a_mint, 500_000, 400_000,
    ).unwrap();
    assert_eq!(&ix.data[..8], &[0x8f, 0xbe, 0x5a, 0xda, 0xc4, 0x1e, 0x33, 0xde]);
}

#[test]
fn test_raydium_cp_swap_ix_returns_none_without_extra() {
    let mut pool = make_test_pool(DexType::RaydiumCp);
    pool.extra = PoolExtra::default(); // no vaults/config
    let signer = Pubkey::new_unique();
    let ix = build_raydium_cp_swap_ix(
        &signer, &pool, pool.token_a_mint, 1_000_000, 900_000,
    );
    assert!(ix.is_none(), "Should return None when PoolExtra fields are missing");
}

#[test]
fn test_damm_v2_swap_ix_account_count() {
    let pool = make_test_pool(DexType::MeteoraDammV2);
    let signer = Pubkey::new_unique();
    let ix = build_damm_v2_swap_ix(
        &signer, &pool, pool.token_a_mint, 1_000_000, 900_000,
    );
    assert!(ix.is_some(), "Should produce an instruction with full PoolExtra");
    let ix = ix.unwrap();
    assert_eq!(ix.accounts.len(), 12, "DAMM v2 swap requires 12 accounts");
}

#[test]
fn test_damm_v2_swap_ix_discriminator() {
    let pool = make_test_pool(DexType::MeteoraDammV2);
    let signer = Pubkey::new_unique();
    let ix = build_damm_v2_swap_ix(
        &signer, &pool, pool.token_a_mint, 500_000, 400_000,
    ).unwrap();
    assert_eq!(&ix.data[..8], &[0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88]);
}

#[test]
fn test_damm_v2_swap_ix_swap_mode_exact_in() {
    let pool = make_test_pool(DexType::MeteoraDammV2);
    let signer = Pubkey::new_unique();
    let ix = build_damm_v2_swap_ix(
        &signer, &pool, pool.token_a_mint, 500_000, 400_000,
    ).unwrap();
    // Data layout: 8 bytes disc + 8 bytes amount_in + 8 bytes min_out + 1 byte swap_mode
    assert_eq!(ix.data.len(), 25, "DAMM v2 data should be 25 bytes");
    assert_eq!(ix.data[24], 0u8, "swap_mode should be 0 (ExactIn)");
}
