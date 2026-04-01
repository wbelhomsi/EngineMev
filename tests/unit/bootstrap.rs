use solana_sdk::pubkey::Pubkey;

use solana_mev_bot::router::pool::DexType;
use solana_mev_bot::state::bootstrap::{
    parse_raydium_amm_pool,
    parse_orca_whirlpool_pool,
    parse_meteora_dlmm_pool,
};

/// Helper: build a fake Raydium AMM account data buffer (752 bytes).
/// Sets status=6 at offset 0, vaults at 336/368, mints at 400/432.
fn make_raydium_data(
    coin_vault: &Pubkey,
    pc_vault: &Pubkey,
    coin_mint: &Pubkey,
    pc_mint: &Pubkey,
) -> Vec<u8> {
    let mut data = vec![0u8; 752];
    data[0..8].copy_from_slice(&6u64.to_le_bytes());
    data[336..368].copy_from_slice(coin_vault.as_ref());
    data[368..400].copy_from_slice(pc_vault.as_ref());
    data[400..432].copy_from_slice(coin_mint.as_ref());
    data[432..464].copy_from_slice(pc_mint.as_ref());
    data
}

/// Helper: build a fake Orca Whirlpool account data buffer (653 bytes).
fn make_whirlpool_data(
    mint_a: &Pubkey,
    vault_a: &Pubkey,
    mint_b: &Pubkey,
    vault_b: &Pubkey,
    sqrt_price: u128,
    tick: i32,
    liquidity: u128,
) -> Vec<u8> {
    let mut data = vec![0u8; 653];
    data[49..65].copy_from_slice(&liquidity.to_le_bytes());
    data[65..81].copy_from_slice(&sqrt_price.to_le_bytes());
    data[81..85].copy_from_slice(&tick.to_le_bytes());
    data[101..133].copy_from_slice(mint_a.as_ref());
    data[133..165].copy_from_slice(vault_a.as_ref());
    data[181..213].copy_from_slice(mint_b.as_ref());
    data[213..245].copy_from_slice(vault_b.as_ref());
    data
}

/// Helper: build a fake Meteora DLMM account data buffer (904 bytes).
fn make_meteora_data(
    mint_x: &Pubkey,
    mint_y: &Pubkey,
    reserve_x: &Pubkey,
    reserve_y: &Pubkey,
    active_id: i32,
    bin_step: u16,
) -> Vec<u8> {
    let mut data = vec![0u8; 904];
    data[76..80].copy_from_slice(&active_id.to_le_bytes());
    data[80..82].copy_from_slice(&bin_step.to_le_bytes());
    data[88..120].copy_from_slice(mint_x.as_ref());
    data[120..152].copy_from_slice(mint_y.as_ref());
    data[152..184].copy_from_slice(reserve_x.as_ref());
    data[184..216].copy_from_slice(reserve_y.as_ref());
    data
}

#[test]
fn test_parse_raydium_amm_pool() {
    let pool_addr = Pubkey::new_unique();
    let coin_vault = Pubkey::new_unique();
    let pc_vault = Pubkey::new_unique();
    let coin_mint = Pubkey::new_unique();
    let pc_mint = Pubkey::new_unique();

    let data = make_raydium_data(&coin_vault, &pc_vault, &coin_mint, &pc_mint);
    let result = parse_raydium_amm_pool(&pool_addr, &data);
    assert!(result.is_some(), "Should parse valid Raydium data");

    let (pool, vault_a, vault_b) = result.unwrap();
    assert_eq!(pool.address, pool_addr);
    assert_eq!(pool.dex_type, DexType::RaydiumAmm);
    assert_eq!(pool.token_a_mint, coin_mint);
    assert_eq!(pool.token_b_mint, pc_mint);
    assert_eq!(vault_a, coin_vault);
    assert_eq!(vault_b, pc_vault);
}

#[test]
fn test_parse_raydium_rejects_short_data() {
    let pool_addr = Pubkey::new_unique();
    let data = vec![0u8; 100];
    assert!(parse_raydium_amm_pool(&pool_addr, &data).is_none());
}

#[test]
fn test_parse_orca_whirlpool() {
    let pool_addr = Pubkey::new_unique();
    let mint_a = Pubkey::new_unique();
    let vault_a = Pubkey::new_unique();
    let mint_b = Pubkey::new_unique();
    let vault_b = Pubkey::new_unique();

    let data = make_whirlpool_data(&mint_a, &vault_a, &mint_b, &vault_b, 1_000_000, -100, 500_000);
    let result = parse_orca_whirlpool_pool(&pool_addr, &data);
    assert!(result.is_some(), "Should parse valid Whirlpool data");

    let (pool, va, vb) = result.unwrap();
    assert_eq!(pool.address, pool_addr);
    assert_eq!(pool.dex_type, DexType::OrcaWhirlpool);
    assert_eq!(pool.token_a_mint, mint_a);
    assert_eq!(pool.token_b_mint, mint_b);
    assert_eq!(pool.sqrt_price_x64, Some(1_000_000));
    assert_eq!(pool.current_tick, Some(-100));
    assert_eq!(pool.liquidity, Some(500_000));
    assert_eq!(va, vault_a);
    assert_eq!(vb, vault_b);
}

#[test]
fn test_parse_meteora_dlmm() {
    let pool_addr = Pubkey::new_unique();
    let mint_x = Pubkey::new_unique();
    let mint_y = Pubkey::new_unique();
    let reserve_x = Pubkey::new_unique();
    let reserve_y = Pubkey::new_unique();

    let data = make_meteora_data(&mint_x, &mint_y, &reserve_x, &reserve_y, 42, 10);
    let result = parse_meteora_dlmm_pool(&pool_addr, &data);
    assert!(result.is_some(), "Should parse valid Meteora data");

    let (pool, vx, vy) = result.unwrap();
    assert_eq!(pool.address, pool_addr);
    assert_eq!(pool.dex_type, DexType::MeteoraDlmm);
    assert_eq!(pool.token_a_mint, mint_x);
    assert_eq!(pool.token_b_mint, mint_y);
    assert_eq!(vx, reserve_x);
    assert_eq!(vy, reserve_y);
}
