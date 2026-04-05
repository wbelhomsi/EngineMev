use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use solana_mev_bot::executor::bundle::build_raydium_amm_swap_ix;
use solana_mev_bot::router::pool::{DexType, PoolState, PoolExtra};

/// Helper: build a PoolState with Raydium AMM v4 fields for Swap V2.
/// Only needs vault_a, vault_b, amm_nonce (no Serum fields).
fn make_raydium_amm_pool() -> PoolState {
    let amm_program = Pubkey::from_str("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8").unwrap();
    let nonce = (0u8..=255).find(|n| {
        Pubkey::create_program_address(&[&[*n]], &amm_program).is_ok()
    }).expect("Should find a valid nonce for AMM authority PDA");

    PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::RaydiumAmm,
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
            token_program_a: Some(Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()),
            token_program_b: Some(Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()),
            amm_nonce: Some(nonce),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    }
}

#[test]
fn test_raydium_amm_v4_swap_ix_8_accounts() {
    let pool = make_raydium_amm_pool();
    let signer = Pubkey::new_unique();
    let ix = build_raydium_amm_swap_ix(
        &signer, &pool, pool.token_a_mint, 1_000_000, 900_000,
    );
    assert!(ix.is_some(), "Should produce an instruction with vault_a + vault_b + nonce");
    let ix = ix.unwrap();
    assert_eq!(ix.accounts.len(), 8, "Raydium AMM v4 Swap V2 requires 8 accounts");

    // First account is SPL Token program (readonly)
    let spl_token = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
    assert_eq!(ix.accounts[0].pubkey, spl_token);
    assert!(!ix.accounts[0].is_writable);
    assert!(!ix.accounts[0].is_signer);

    // Last account is signer
    assert_eq!(ix.accounts[7].pubkey, signer);
    assert!(ix.accounts[7].is_signer);
    assert!(!ix.accounts[7].is_writable);

    // Instruction data is [16, amount_in(u64 LE), min_out(u64 LE)] = 17 bytes
    assert_eq!(ix.data.len(), 17);
    assert_eq!(ix.data[0], 16u8, "swap_base_in_v2 discriminator");
    let amount_in = u64::from_le_bytes(ix.data[1..9].try_into().unwrap());
    assert_eq!(amount_in, 1_000_000);
    let min_out = u64::from_le_bytes(ix.data[9..17].try_into().unwrap());
    assert_eq!(min_out, 900_000);
}

#[test]
fn test_raydium_amm_v4_swap_v2_no_serum_required() {
    // Build IX with vault_a, vault_b, amm_nonce but NO serum fields at all.
    // V2 must succeed without any Serum/OpenBook data.
    let pool = make_raydium_amm_pool();
    // Confirm no serum fields are set (default is None)
    assert!(pool.extra.open_orders.is_none());
    assert!(pool.extra.market.is_none());
    assert!(pool.extra.market_program.is_none());
    assert!(pool.extra.target_orders.is_none());

    let signer = Pubkey::new_unique();
    let ix = build_raydium_amm_swap_ix(
        &signer, &pool, pool.token_a_mint, 500_000, 400_000,
    );
    assert!(ix.is_some(), "V2 should succeed without any Serum fields");
}

#[test]
fn test_raydium_amm_v4_swap_v2_discriminator_is_16() {
    let pool = make_raydium_amm_pool();
    let signer = Pubkey::new_unique();
    let ix = build_raydium_amm_swap_ix(
        &signer, &pool, pool.token_a_mint, 1_000_000, 900_000,
    ).expect("Should build V2 IX");
    assert_eq!(ix.data[0], 16u8, "SwapBaseInV2 discriminator must be 16");
}

#[test]
fn test_raydium_amm_v4_swap_ix_returns_none_without_nonce() {
    let mut pool = make_raydium_amm_pool();
    pool.extra.amm_nonce = None;
    let signer = Pubkey::new_unique();
    let ix = build_raydium_amm_swap_ix(
        &signer, &pool, pool.token_a_mint, 1_000_000, 900_000,
    );
    assert!(ix.is_none(), "Should return None when amm_nonce is missing");
}

#[test]
fn test_raydium_amm_v4_swap_v2_account_ordering() {
    let pool = make_raydium_amm_pool();
    let signer = Pubkey::new_unique();
    let ix = build_raydium_amm_swap_ix(
        &signer, &pool, pool.token_a_mint, 1_000_000, 900_000,
    ).expect("Should build V2 IX");

    let spl_token = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
    let amm_program = Pubkey::from_str("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8").unwrap();
    let nonce = pool.extra.amm_nonce.unwrap();
    let amm_authority = Pubkey::create_program_address(&[&[nonce]], &amm_program).unwrap();

    // [0] SPL Token program
    assert_eq!(ix.accounts[0].pubkey, spl_token);
    // [1] amm_id
    assert_eq!(ix.accounts[1].pubkey, pool.address);
    // [2] amm_authority
    assert_eq!(ix.accounts[2].pubkey, amm_authority);
    // [3] pool_coin_token_account (vault_a)
    assert_eq!(ix.accounts[3].pubkey, pool.extra.vault_a.unwrap());
    // [4] pool_pc_token_account (vault_b)
    assert_eq!(ix.accounts[4].pubkey, pool.extra.vault_b.unwrap());
    // [5] user_source — ATA of signer for input_mint
    // [6] user_dest — ATA of signer for output_mint
    // [7] signer
    assert_eq!(ix.accounts[7].pubkey, signer);
}
