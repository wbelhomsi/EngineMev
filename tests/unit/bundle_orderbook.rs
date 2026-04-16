use solana_sdk::pubkey::Pubkey;
use solana_mev_bot::executor::swaps::{build_phoenix_swap_ix, build_manifest_swap_ix};
use solana_mev_bot::router::pool::{DexType, PoolState, PoolExtra};

fn make_phoenix_pool() -> PoolState {
    PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::Phoenix,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1000,
        token_b_reserve: 1000,
        fee_bps: 2,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            ..Default::default()
        },
        best_bid_price: Some(150),
        best_ask_price: Some(160),
    }
}

fn make_manifest_pool() -> PoolState {
    PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::Manifest,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1000,
        token_b_reserve: 1000,
        fee_bps: 0,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            ..Default::default()
        },
        best_bid_price: Some(150),
        best_ask_price: Some(160),
    }
}

// Phoenix tests
#[test]
fn test_build_phoenix_swap_ix_returns_some() {
    let signer = Pubkey::new_unique();
    let pool = make_phoenix_pool();
    let result = build_phoenix_swap_ix(&signer, &pool, pool.token_a_mint, 100, 90);
    assert!(result.is_some());
}

#[test]
fn test_build_phoenix_swap_ix_has_correct_program_id() {
    let signer = Pubkey::new_unique();
    let pool = make_phoenix_pool();
    let ix = build_phoenix_swap_ix(&signer, &pool, pool.token_a_mint, 100, 90).unwrap();
    let expected: Pubkey = "PhoeNiXZ8ByJGLkxNfZRnkUfjvmuYqLR89jjFHGqdXY".parse().unwrap();
    assert_eq!(ix.program_id, expected);
}

#[test]
fn test_build_phoenix_swap_ix_has_9_accounts() {
    let signer = Pubkey::new_unique();
    let pool = make_phoenix_pool();
    let ix = build_phoenix_swap_ix(&signer, &pool, pool.token_a_mint, 100, 90).unwrap();
    assert_eq!(ix.accounts.len(), 9);
}

#[test]
fn test_build_phoenix_swap_ix_missing_vaults() {
    let signer = Pubkey::new_unique();
    let mut pool = make_phoenix_pool();
    pool.extra.vault_a = None;
    assert!(build_phoenix_swap_ix(&signer, &pool, pool.token_a_mint, 100, 90).is_none());
}

// Manifest tests
#[test]
fn test_build_manifest_swap_ix_returns_some() {
    let signer = Pubkey::new_unique();
    let pool = make_manifest_pool();
    assert!(build_manifest_swap_ix(&signer, &pool, pool.token_a_mint, 100, 90).is_some());
}

#[test]
fn test_build_manifest_swap_ix_has_correct_program_id() {
    let signer = Pubkey::new_unique();
    let pool = make_manifest_pool();
    let ix = build_manifest_swap_ix(&signer, &pool, pool.token_a_mint, 100, 90).unwrap();
    let expected: Pubkey = "MNFSTqtC93rEfYHB6hF82sKdZpUDFWkViLByLd1k1Ms".parse().unwrap();
    assert_eq!(ix.program_id, expected);
}

#[test]
fn test_build_manifest_swap_ix_has_8_accounts() {
    let signer = Pubkey::new_unique();
    let pool = make_manifest_pool();
    let ix = build_manifest_swap_ix(&signer, &pool, pool.token_a_mint, 100, 90).unwrap();
    assert_eq!(ix.accounts.len(), 8);
}

#[test]
fn test_build_manifest_swap_ix_missing_vaults() {
    let signer = Pubkey::new_unique();
    let mut pool = make_manifest_pool();
    pool.extra.vault_b = None;
    assert!(build_manifest_swap_ix(&signer, &pool, pool.token_a_mint, 100, 90).is_none());
}
