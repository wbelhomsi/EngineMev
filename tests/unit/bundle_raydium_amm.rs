use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use solana_mev_bot::executor::bundle::build_raydium_amm_swap_ix;
use solana_mev_bot::router::pool::{DexType, PoolState, PoolExtra};

/// Helper: build a PoolState with all Raydium AMM v4 + Serum fields populated.
/// Uses nonce=254 which is the most common AMM authority nonce on mainnet.
fn make_raydium_amm_pool() -> PoolState {
    // Use a known nonce that produces a valid PDA.
    // Raydium AMM authority PDA: seeds=[&[nonce]], program=RAYDIUM_AMM
    // We'll try nonce values until we find one that works.
    let amm_program = Pubkey::from_str("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8").unwrap();
    let nonce = (0u8..=255).find(|n| {
        Pubkey::create_program_address(&[&[*n]], &amm_program).is_ok()
    }).expect("Should find a valid nonce for AMM authority PDA");

    // For the serum vault signer PDA, we need a market_id and nonce that produce a valid PDA.
    // Use a known market_program and find a valid nonce.
    let market_program = Pubkey::from_str("srmqPvymJeFKQ4zGQed1GFppgkRHL9kaELCbyksJtPX").unwrap();
    let market_id = Pubkey::new_unique();
    let serum_nonce = (0u64..=255).find(|n| {
        Pubkey::create_program_address(
            &[market_id.as_ref(), &n.to_le_bytes()],
            &market_program,
        ).is_ok()
    }).expect("Should find a valid serum vault signer nonce");

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
            open_orders: Some(Pubkey::new_unique()),
            market: Some(market_id),
            market_program: Some(market_program),
            target_orders: Some(Pubkey::new_unique()),
            amm_nonce: Some(nonce),
            serum_bids: Some(Pubkey::new_unique()),
            serum_asks: Some(Pubkey::new_unique()),
            serum_event_queue: Some(Pubkey::new_unique()),
            serum_coin_vault: Some(Pubkey::new_unique()),
            serum_pc_vault: Some(Pubkey::new_unique()),
            serum_vault_signer_nonce: Some(serum_nonce),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    }
}

#[test]
fn test_raydium_amm_v4_swap_ix_18_accounts() {
    let pool = make_raydium_amm_pool();
    let signer = Pubkey::new_unique();
    let ix = build_raydium_amm_swap_ix(
        &signer, &pool, pool.token_a_mint, 1_000_000, 900_000,
    );
    assert!(ix.is_some(), "Should produce an instruction with full PoolExtra + Serum fields");
    let ix = ix.unwrap();
    assert_eq!(ix.accounts.len(), 18, "Raydium AMM v4 swap requires 18 accounts");

    // First account is SPL Token program (readonly)
    let spl_token = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
    assert_eq!(ix.accounts[0].pubkey, spl_token);
    assert!(!ix.accounts[0].is_writable);
    assert!(!ix.accounts[0].is_signer);

    // Last account is signer
    assert_eq!(ix.accounts[17].pubkey, signer);
    assert!(ix.accounts[17].is_signer);
    assert!(!ix.accounts[17].is_writable);

    // Instruction data is [9, amount_in(u64 LE), min_out(u64 LE)] = 17 bytes
    assert_eq!(ix.data.len(), 17);
    assert_eq!(ix.data[0], 9u8, "swap_base_in discriminator");
    let amount_in = u64::from_le_bytes(ix.data[1..9].try_into().unwrap());
    assert_eq!(amount_in, 1_000_000);
    let min_out = u64::from_le_bytes(ix.data[9..17].try_into().unwrap());
    assert_eq!(min_out, 900_000);
}

#[test]
fn test_raydium_amm_v4_swap_ix_returns_none_without_serum() {
    let mut pool = make_raydium_amm_pool();
    // Clear serum accounts — simulate pool that hasn't had market fetched yet
    pool.extra.serum_bids = None;
    pool.extra.serum_asks = None;
    pool.extra.serum_event_queue = None;
    pool.extra.serum_coin_vault = None;
    pool.extra.serum_pc_vault = None;
    pool.extra.serum_vault_signer_nonce = None;
    let signer = Pubkey::new_unique();
    let ix = build_raydium_amm_swap_ix(
        &signer, &pool, pool.token_a_mint, 1_000_000, 900_000,
    );
    assert!(ix.is_none(), "Should return None when Serum accounts are missing");
}

#[test]
fn test_raydium_amm_v4_swap_ix_returns_none_without_open_orders() {
    let mut pool = make_raydium_amm_pool();
    pool.extra.open_orders = None;
    let signer = Pubkey::new_unique();
    let ix = build_raydium_amm_swap_ix(
        &signer, &pool, pool.token_a_mint, 1_000_000, 900_000,
    );
    assert!(ix.is_none(), "Should return None when open_orders is missing");
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
