use solana_sdk::pubkey::Pubkey;
use crate::router::pool::{DexType, PoolExtra, PoolState};
use super::approx_reserves_from_sqrt_price;

/// Parse a Meteora DAMM v2 pool account (1112 bytes).
///
/// Layout (byte offsets):
///   0   discriminator (8 bytes): [241, 154, 109, 4, 17, 177, 109, 188]
///   168 token_a_mint (Pubkey, 32)
///   200 token_b_mint (Pubkey, 32)
///   232 a_vault (Pubkey, 32)
///   264 b_vault (Pubkey, 32)
///   360 liquidity (u128, 16)  — used for concentrated mode
///   456 sqrt_price (u128, 16) — used for concentrated mode
///   484 collect_fee_mode (u8): 4 = compounding (direct reserves), 0-3 = concentrated
///   680 token_a_amount (u64, 8) — used when collect_fee_mode == 4
///   688 token_b_amount (u64, 8) — used when collect_fee_mode == 4
pub fn parse_meteora_damm_v2(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<PoolState> {
    const MIN_LEN: usize = 696;
    if data.len() < MIN_LEN {
        return None;
    }

    let mint_a = Pubkey::new_from_array(data[168..200].try_into().ok()?);
    let mint_b = Pubkey::new_from_array(data[200..232].try_into().ok()?);
    let collect_fee_mode = data[484];

    let (reserve_a, reserve_b, sqrt_price_x64, liquidity) = if collect_fee_mode == 4 {
        // Compounding mode: direct reserves stored in account
        let ra = u64::from_le_bytes(data[680..688].try_into().ok()?);
        let rb = u64::from_le_bytes(data[688..696].try_into().ok()?);
        (ra, rb, None, None)
    } else {
        // Concentrated mode: derive from sqrt_price + liquidity
        // Both fields require data.len() >= 472
        if data.len() < 472 {
            return None;
        }
        let liq = u128::from_le_bytes(data[360..376].try_into().ok()?);
        let sqrt_p = u128::from_le_bytes(data[456..472].try_into().ok()?);
        let (ra, rb) = approx_reserves_from_sqrt_price(sqrt_p, liq);
        (ra, rb, Some(sqrt_p), Some(liq))
    };

    Some(PoolState {
        address: *pool_address,
        dex_type: DexType::MeteoraDammV2,
        token_a_mint: mint_a,
        token_b_mint: mint_b,
        token_a_reserve: reserve_a,
        token_b_reserve: reserve_b,
        fee_bps: DexType::MeteoraDammV2.base_fee_bps(),
        current_tick: None,
        sqrt_price_x64,
        liquidity,
        last_slot: slot,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_from_array(data[232..264].try_into().ok()?)),
            vault_b: Some(Pubkey::new_from_array(data[264..296].try_into().ok()?)),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    })
}
