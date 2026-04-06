use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use solana_mev_bot::executor::bundle::{
    build_raydium_cp_swap_ix, build_damm_v2_swap_ix,
    build_orca_whirlpool_swap_ix, build_raydium_clmm_swap_ix, build_meteora_dlmm_swap_ix,
    build_raydium_amm_swap_ix, build_pumpswap_swap_ix,
};
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
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
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

// ─── Orca Whirlpool tests ───────────────────────────────────────────────────

#[test]
fn test_orca_whirlpool_swap_ix_account_count() {
    let pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1_000_000,
        token_b_reserve: 1_000_000,
        fee_bps: 30,
        current_tick: Some(100),
        sqrt_price_x64: Some(1_000_000),
        liquidity: Some(500_000),
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
    let signer = Pubkey::new_unique();
    let ix = build_orca_whirlpool_swap_ix(&signer, &pool, pool.token_a_mint, 1000, 900);
    assert!(ix.is_some(), "Should produce an instruction with vaults + tick_spacing");
    let ix = ix.unwrap();
    assert_eq!(ix.accounts.len(), 15, "Orca swap_v2 needs 15 accounts");
}

#[test]
fn test_orca_whirlpool_swap_ix_discriminator() {
    let pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1_000_000,
        token_b_reserve: 1_000_000,
        fee_bps: 30,
        current_tick: Some(100),
        sqrt_price_x64: Some(1_000_000),
        liquidity: Some(500_000),
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
    let signer = Pubkey::new_unique();
    let ix = build_orca_whirlpool_swap_ix(&signer, &pool, pool.token_a_mint, 1000, 900).unwrap();
    assert_eq!(&ix.data[0..8], &[0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62]);
}

#[test]
fn test_orca_whirlpool_swap_ix_returns_none_without_tick_spacing() {
    let pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1_000_000,
        token_b_reserve: 1_000_000,
        fee_bps: 30,
        current_tick: Some(100),
        sqrt_price_x64: Some(1_000_000),
        liquidity: Some(500_000),
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            ..Default::default() // tick_spacing is None
        },
        best_bid_price: None,
        best_ask_price: None,
    };
    let signer = Pubkey::new_unique();
    let ix = build_orca_whirlpool_swap_ix(&signer, &pool, pool.token_a_mint, 1000, 900);
    assert!(ix.is_none(), "Should return None when tick_spacing is missing");
}

// ─── Raydium CLMM tests ────────────────────────────────────────────────────

#[test]
fn test_raydium_clmm_swap_ix_account_count() {
    let pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::RaydiumClmm,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1_000_000,
        token_b_reserve: 1_000_000,
        fee_bps: 25,
        current_tick: Some(50),
        sqrt_price_x64: Some(1_000_000),
        liquidity: Some(500_000),
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            config: Some(Pubkey::new_unique()),
            observation: Some(Pubkey::new_unique()),
            tick_spacing: Some(10),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    };
    let signer = Pubkey::new_unique();
    let ix = build_raydium_clmm_swap_ix(&signer, &pool, pool.token_a_mint, 1000, 900);
    assert!(ix.is_some(), "Should produce an instruction with full CLMM extra");
    let ix = ix.unwrap();
    assert_eq!(ix.accounts.len(), 17, "Raydium CLMM swap_v2 needs 17 accounts");
}

#[test]
fn test_raydium_clmm_swap_ix_discriminator() {
    let pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::RaydiumClmm,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1_000_000,
        token_b_reserve: 1_000_000,
        fee_bps: 25,
        current_tick: Some(50),
        sqrt_price_x64: Some(1_000_000),
        liquidity: Some(500_000),
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            config: Some(Pubkey::new_unique()),
            observation: Some(Pubkey::new_unique()),
            tick_spacing: Some(10),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    };
    let signer = Pubkey::new_unique();
    let ix = build_raydium_clmm_swap_ix(&signer, &pool, pool.token_a_mint, 1000, 900).unwrap();
    assert_eq!(&ix.data[0..8], &[0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62]);
}

#[test]
fn test_raydium_clmm_swap_ix_returns_none_without_observation() {
    let pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::RaydiumClmm,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1_000_000,
        token_b_reserve: 1_000_000,
        fee_bps: 25,
        current_tick: Some(50),
        sqrt_price_x64: Some(1_000_000),
        liquidity: Some(500_000),
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            config: Some(Pubkey::new_unique()),
            tick_spacing: Some(10),
            // observation is None
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    };
    let signer = Pubkey::new_unique();
    let ix = build_raydium_clmm_swap_ix(&signer, &pool, pool.token_a_mint, 1000, 900);
    assert!(ix.is_none(), "Should return None when observation is missing");
}

// ─── Meteora DLMM tests ────────────────────────────────────────────────────

#[test]
fn test_meteora_dlmm_swap_ix_account_count() {
    let pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::MeteoraDlmm,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1_000_000,
        token_b_reserve: 1_000_000,
        fee_bps: 10,
        current_tick: Some(100),
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            bitmap_extension: Some(Pubkey::new_unique()),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    };
    let signer = Pubkey::new_unique();
    let ix = build_meteora_dlmm_swap_ix(&signer, &pool, pool.token_a_mint, 2000, 1800, None, None);
    assert!(ix.is_some(), "Should produce an instruction with bitmap_extension");
    let ix = ix.unwrap();
    // 16 fixed + 3 bin arrays = 19 (includes memo program)
    assert_eq!(ix.accounts.len(), 19, "DLMM swap2 needs 16 fixed + 3 bin arrays");
}

#[test]
fn test_meteora_dlmm_swap_ix_discriminator() {
    let pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::MeteoraDlmm,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1_000_000,
        token_b_reserve: 1_000_000,
        fee_bps: 10,
        current_tick: Some(100),
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            bitmap_extension: Some(Pubkey::new_unique()),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    };
    let signer = Pubkey::new_unique();
    let ix = build_meteora_dlmm_swap_ix(&signer, &pool, pool.token_a_mint, 2000, 1800, None, None).unwrap();
    assert_eq!(&ix.data[0..8], &[0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88]);
}

#[test]
fn test_meteora_dlmm_swap_ix_returns_none_without_vaults() {
    let pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::MeteoraDlmm,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1_000_000,
        token_b_reserve: 1_000_000,
        fee_bps: 10,
        current_tick: Some(100),
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(), // no vaults
        best_bid_price: None,
        best_ask_price: None,
    };
    let signer = Pubkey::new_unique();
    let ix = build_meteora_dlmm_swap_ix(&signer, &pool, pool.token_a_mint, 2000, 1800, None, None);
    assert!(ix.is_none(), "Should return None when vaults are missing");
}

// ---- Raydium AMM v4 ----

fn make_raydium_amm_pool() -> PoolState {
    let amm_program = Pubkey::from_str("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8").unwrap();
    let nonce = (0u8..=255).find(|n| {
        Pubkey::create_program_address(&[&[*n]], &amm_program).is_ok()
    }).expect("valid AMM nonce");

    PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::RaydiumAmm,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1_000_000,
        token_b_reserve: 1_000_000,
        fee_bps: 25,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            amm_nonce: Some(nonce),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    }
}

#[test]
fn test_raydium_amm_swap_ix_account_count() {
    let pool = make_raydium_amm_pool();
    let signer = Pubkey::new_unique();
    let ix = build_raydium_amm_swap_ix(&signer, &pool, pool.token_a_mint, 1000, 900).unwrap();
    assert_eq!(ix.accounts.len(), 8, "Raydium AMM v4 Swap V2 needs 8 accounts");
}

#[test]
fn test_raydium_amm_swap_ix_discriminator() {
    let pool = make_raydium_amm_pool();
    let signer = Pubkey::new_unique();
    let ix = build_raydium_amm_swap_ix(&signer, &pool, pool.token_a_mint, 5000, 4500).unwrap();
    assert_eq!(ix.data[0], 16u8, "Discriminator must be 16 (SwapBaseInV2)");
    assert_eq!(ix.data.len(), 17, "Data must be 17 bytes: 1 disc + 8 amount_in + 8 min_out");
    let amount_in = u64::from_le_bytes(ix.data[1..9].try_into().unwrap());
    assert_eq!(amount_in, 5000);
    let min_out = u64::from_le_bytes(ix.data[9..17].try_into().unwrap());
    assert_eq!(min_out, 4500);
}

#[test]
fn test_raydium_amm_swap_ix_returns_none_without_vaults() {
    let pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::RaydiumAmm,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 1_000_000,
        token_b_reserve: 1_000_000,
        fee_bps: 25,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra::default(), // no vaults, no nonce
        best_bid_price: None,
        best_ask_price: None,
    };
    let signer = Pubkey::new_unique();
    let ix = build_raydium_amm_swap_ix(&signer, &pool, pool.token_a_mint, 1000, 900);
    assert!(ix.is_none(), "Should return None when PoolExtra is empty");
}

// ─── PumpSwap tests ────────────────────────────────────────────────────────

fn make_pumpswap_pool() -> PoolState {
    PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::PumpSwap,
        token_a_mint: Pubkey::new_unique(), // base (memecoin)
        token_b_mint: Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap(), // wSOL
        token_a_reserve: 1_000_000_000,
        token_b_reserve: 5_000_000_000,
        fee_bps: 125,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            coin_creator: Some(Pubkey::new_unique()),
            is_mayhem_mode: Some(false),
            is_cashback_coin: Some(false),
            token_program_a: Some(Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()),
            token_program_b: Some(Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    }
}

#[test]
fn test_pumpswap_sell_ix_account_count() {
    let pool = make_pumpswap_pool();
    let signer = Pubkey::new_unique();
    // Sell: input is base (token_a_mint)
    let ix = build_pumpswap_swap_ix(&signer, &pool, pool.token_a_mint, 1_000_000, 900_000);
    assert!(ix.is_some(), "Should produce an instruction with full PumpSwap extra");
    let ix = ix.unwrap();
    assert_eq!(ix.accounts.len(), 21, "PumpSwap sell requires 21 accounts");
}

#[test]
fn test_pumpswap_sell_ix_discriminator() {
    let pool = make_pumpswap_pool();
    let signer = Pubkey::new_unique();
    let ix = build_pumpswap_swap_ix(&signer, &pool, pool.token_a_mint, 500_000, 400_000).unwrap();
    assert_eq!(&ix.data[0..8], &[51, 230, 133, 164, 1, 127, 131, 173]);
}

#[test]
fn test_pumpswap_buy_ix_account_count() {
    let pool = make_pumpswap_pool();
    let signer = Pubkey::new_unique();
    // Buy: input is quote (token_b_mint / wSOL)
    let ix = build_pumpswap_swap_ix(&signer, &pool, pool.token_b_mint, 1_000_000, 900_000);
    assert!(ix.is_some(), "Should produce a buy instruction");
    let ix = ix.unwrap();
    assert_eq!(ix.accounts.len(), 23, "PumpSwap buy requires 23 accounts");
}

#[test]
fn test_pumpswap_buy_ix_discriminator() {
    let pool = make_pumpswap_pool();
    let signer = Pubkey::new_unique();
    let ix = build_pumpswap_swap_ix(&signer, &pool, pool.token_b_mint, 500_000, 400_000).unwrap();
    assert_eq!(&ix.data[0..8], &[102, 6, 61, 18, 1, 218, 235, 234]);
}

#[test]
fn test_pumpswap_ix_returns_none_without_vaults() {
    let mut pool = make_pumpswap_pool();
    pool.extra.vault_a = None;
    pool.extra.vault_b = None;
    let signer = Pubkey::new_unique();
    let ix = build_pumpswap_swap_ix(&signer, &pool, pool.token_a_mint, 1_000_000, 900_000);
    assert!(ix.is_none(), "Should return None when vaults are missing");
}

#[test]
fn test_pumpswap_ix_returns_none_without_coin_creator() {
    let mut pool = make_pumpswap_pool();
    pool.extra.coin_creator = None;
    let signer = Pubkey::new_unique();
    let ix = build_pumpswap_swap_ix(&signer, &pool, pool.token_a_mint, 1_000_000, 900_000);
    assert!(ix.is_none(), "Should return None when coin_creator is missing");
}
