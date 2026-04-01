use solana_sdk::pubkey::Pubkey;

use crate::router::pool::{DexType, PoolState};

/// Parse a Raydium AMM v4 pool account.
/// Returns (PoolState, vault_a_pubkey, vault_b_pubkey) or None if data is invalid.
///
/// Layout (no Anchor discriminator, 752 bytes):
///   offset 336: coin_vault (Pubkey, 32B)
///   offset 368: pc_vault (Pubkey, 32B)
///   offset 400: coin_vault_mint (Pubkey, 32B)
///   offset 432: pc_vault_mint (Pubkey, 32B)
pub fn parse_raydium_amm_pool(
    pool_address: &Pubkey,
    data: &[u8],
) -> Option<(PoolState, Pubkey, Pubkey)> {
    if data.len() < 464 {
        return None;
    }

    let coin_vault = Pubkey::try_from(&data[336..368]).ok()?;
    let pc_vault = Pubkey::try_from(&data[368..400]).ok()?;
    let coin_mint = Pubkey::try_from(&data[400..432]).ok()?;
    let pc_mint = Pubkey::try_from(&data[432..464]).ok()?;

    let pool = PoolState {
        address: *pool_address,
        dex_type: DexType::RaydiumAmm,
        token_a_mint: coin_mint,
        token_b_mint: pc_mint,
        token_a_reserve: 0,
        token_b_reserve: 0,
        fee_bps: DexType::RaydiumAmm.base_fee_bps(),
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 0,
    };

    Some((pool, coin_vault, pc_vault))
}

/// Parse an Orca Whirlpool pool account.
/// Returns (PoolState, vault_a_pubkey, vault_b_pubkey) or None if data is invalid.
///
/// Layout (8-byte Anchor discriminator, 653 bytes total):
///   offset 49: liquidity (u128 LE, 16B)
///   offset 65: sqrt_price (u128 LE, 16B)
///   offset 81: tick_current_index (i32 LE, 4B)
///   offset 101: token_mint_a (Pubkey, 32B)
///   offset 133: token_vault_a (Pubkey, 32B)
///   offset 181: token_mint_b (Pubkey, 32B)
///   offset 213: token_vault_b (Pubkey, 32B)
pub fn parse_orca_whirlpool_pool(
    pool_address: &Pubkey,
    data: &[u8],
) -> Option<(PoolState, Pubkey, Pubkey)> {
    if data.len() < 245 {
        return None;
    }

    let liquidity = u128::from_le_bytes(data[49..65].try_into().ok()?);
    let sqrt_price = u128::from_le_bytes(data[65..81].try_into().ok()?);
    let tick = i32::from_le_bytes(data[81..85].try_into().ok()?);
    let mint_a = Pubkey::try_from(&data[101..133]).ok()?;
    let vault_a = Pubkey::try_from(&data[133..165]).ok()?;
    let mint_b = Pubkey::try_from(&data[181..213]).ok()?;
    let vault_b = Pubkey::try_from(&data[213..245]).ok()?;

    let pool = PoolState {
        address: *pool_address,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: mint_a,
        token_b_mint: mint_b,
        token_a_reserve: 0,
        token_b_reserve: 0,
        fee_bps: DexType::OrcaWhirlpool.base_fee_bps(),
        current_tick: Some(tick),
        sqrt_price_x64: Some(sqrt_price),
        liquidity: Some(liquidity),
        last_slot: 0,
    };

    Some((pool, vault_a, vault_b))
}

/// Parse a Meteora DLMM LbPair account.
/// Returns (PoolState, reserve_x_pubkey, reserve_y_pubkey) or None if data is invalid.
///
/// Layout (8-byte Anchor discriminator, ~920 bytes):
///   offset 76: active_id (i32 LE, 4B)
///   offset 80: bin_step (u16 LE, 2B)
///   offset 88: token_x_mint (Pubkey, 32B)
///   offset 120: token_y_mint (Pubkey, 32B)
///   offset 152: reserve_x (Pubkey, 32B)
///   offset 184: reserve_y (Pubkey, 32B)
pub fn parse_meteora_dlmm_pool(
    pool_address: &Pubkey,
    data: &[u8],
) -> Option<(PoolState, Pubkey, Pubkey)> {
    if data.len() < 216 {
        return None;
    }

    let _active_id = i32::from_le_bytes(data[76..80].try_into().ok()?);
    let _bin_step = u16::from_le_bytes(data[80..82].try_into().ok()?);
    let mint_x = Pubkey::try_from(&data[88..120]).ok()?;
    let mint_y = Pubkey::try_from(&data[120..152]).ok()?;
    let reserve_x = Pubkey::try_from(&data[152..184]).ok()?;
    let reserve_y = Pubkey::try_from(&data[184..216]).ok()?;

    let pool = PoolState {
        address: *pool_address,
        dex_type: DexType::MeteoraDlmm,
        token_a_mint: mint_x,
        token_b_mint: mint_y,
        token_a_reserve: 0,
        token_b_reserve: 0,
        fee_bps: DexType::MeteoraDlmm.base_fee_bps(),
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 0,
    };

    Some((pool, reserve_x, reserve_y))
}
