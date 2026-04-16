use solana_sdk::pubkey::Pubkey;
use crate::addresses;
use crate::router::pool::{DexType, PoolExtra, PoolState};
use super::approx_reserves_from_sqrt_price;

/// Parse an Orca Whirlpool pool account (653 bytes).
///
/// Layout (byte offsets):
///   8   discriminator
///   8+1 whirlpools_config (32)
///   49  liquidity (u128, 16 bytes)
///   65  sqrt_price_x64 (u128, 16 bytes)
///   81  tick_current_index (i32, 4 bytes)
///   85  fee_rate (u16), protocol_fee_rate (u16) → 4 bytes
///   89  token_a_protocol_fee (u64) + token_b_protocol_fee (u64) → 16 bytes skip
///   (=105 token_a fees end; however mint_a lands at 101 in practice — use spec offsets)
///   101 token_mint_a (Pubkey, 32 bytes)
///   133 token_vault_a (Pubkey, 32 bytes)
///   181 token_mint_b (Pubkey, 32 bytes)
///   213 token_vault_b (Pubkey, 32 bytes)
pub fn parse_orca_whirlpool(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<PoolState> {
    const MIN_LEN: usize = 245;
    if data.len() < MIN_LEN {
        return None;
    }

    let tick_spacing = u16::from_le_bytes(data[41..43].try_into().ok()?);
    let liquidity = u128::from_le_bytes(data[49..65].try_into().ok()?);
    let sqrt_price_x64 = u128::from_le_bytes(data[65..81].try_into().ok()?);
    let tick = i32::from_le_bytes(data[81..85].try_into().ok()?);
    let mint_a = Pubkey::new_from_array(data[101..133].try_into().ok()?);
    let mint_b = Pubkey::new_from_array(data[181..213].try_into().ok()?);

    let (reserve_a, reserve_b) = approx_reserves_from_sqrt_price(sqrt_price_x64, liquidity);

    Some(PoolState {
        address: *pool_address,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: mint_a,
        token_b_mint: mint_b,
        token_a_reserve: reserve_a,
        token_b_reserve: reserve_b,
        fee_bps: {
            // fee_rate at offset 45, u16, units of 1/1,000,000 (3000 = 0.3% = 30 bps)
            let fee_rate = u16::from_le_bytes(data[45..47].try_into().ok()?) as u64;
            fee_rate / 100 // convert to bps
        },
        current_tick: Some(tick),
        sqrt_price_x64: Some(sqrt_price_x64),
        liquidity: Some(liquidity),
        last_slot: slot,
        extra: {
            let spl_token = addresses::SPL_TOKEN;
            PoolExtra {
                vault_a: Some(Pubkey::new_from_array(data[133..165].try_into().ok()?)),
                vault_b: Some(Pubkey::new_from_array(data[213..245].try_into().ok()?)),
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
