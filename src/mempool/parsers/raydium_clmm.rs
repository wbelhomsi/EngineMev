use solana_sdk::pubkey::Pubkey;
use crate::addresses;
use crate::router::pool::{DexType, PoolExtra, PoolState};
use super::approx_reserves_from_sqrt_price;

/// Parse a Raydium CLMM pool account (1560 bytes).
///
/// Layout (byte offsets):
///   8   discriminator
///   8   bump + padding → 16 total before amm_config
///   16  amm_config (32) → ends at 48
///   48  owner (32) → ends at 80 ← but spec says mint_0 at 73 → use spec
///   73  token_mint_0 (Pubkey, 32)
///   105 token_mint_1 (Pubkey, 32)
///   137 token_vault_0 (Pubkey, 32)
///   169 token_vault_1 (Pubkey, 32)
///   237 liquidity (u128, 16)
///   253 sqrt_price_x64 (u128, 16)
///   269 tick_current (i32, 4)
pub fn parse_raydium_clmm(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<PoolState> {
    const MIN_LEN: usize = 273;
    if data.len() < MIN_LEN {
        return None;
    }

    let amm_config = Pubkey::try_from(&data[9..41]).ok()?;
    let mint_0 = Pubkey::new_from_array(data[73..105].try_into().ok()?);
    let mint_1 = Pubkey::new_from_array(data[105..137].try_into().ok()?);
    let observation_key = Pubkey::try_from(&data[201..233]).ok()?;
    let tick_spacing = u16::from_le_bytes(data[235..237].try_into().ok()?);
    let liquidity = u128::from_le_bytes(data[237..253].try_into().ok()?);
    let sqrt_price_x64 = u128::from_le_bytes(data[253..269].try_into().ok()?);
    let tick = i32::from_le_bytes(data[269..273].try_into().ok()?);

    let (reserve_a, reserve_b) = approx_reserves_from_sqrt_price(sqrt_price_x64, liquidity);

    Some(PoolState {
        address: *pool_address,
        dex_type: DexType::RaydiumClmm,
        token_a_mint: mint_0,
        token_b_mint: mint_1,
        token_a_reserve: reserve_a,
        token_b_reserve: reserve_b,
        fee_bps: 25, // Default 25 bps (0.25%) — most common CLMM fee tier. Actual fee is in amm_config account.
        current_tick: Some(tick),
        sqrt_price_x64: Some(sqrt_price_x64),
        liquidity: Some(liquidity),
        last_slot: slot,
        extra: {
            let spl_token = addresses::SPL_TOKEN;
            PoolExtra {
                vault_a: Some(Pubkey::new_from_array(data[137..169].try_into().ok()?)),
                vault_b: Some(Pubkey::new_from_array(data[169..201].try_into().ok()?)),
                config: Some(amm_config),
                observation: Some(observation_key),
                tick_spacing: Some(tick_spacing),
                token_program_a: Some(spl_token),
                token_program_b: Some(spl_token),
                ..Default::default()
            }
        },
        best_bid_price: None,
        best_ask_price: None,
    })
}
