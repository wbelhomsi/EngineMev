use solana_sdk::pubkey::Pubkey;
use crate::addresses;
use crate::router::pool::{DexType, PoolExtra, PoolState};

/// Parse a Meteora DLMM pool account (904 bytes).
///
/// Layout (byte offsets):
///   76  active_id (i32, 4)
///   80  bin_step (u16, 2)
///   88  token_x_mint (Pubkey, 32)
///   120 token_y_mint (Pubkey, 32)
///   152 reserve_x (Pubkey vault, 32)
///   184 reserve_y (Pubkey vault, 32)
///
/// Price = (1 + bin_step/10000)^active_id. Synthetic reserves derived from
/// this price and an assumed unit liquidity for route discovery.
pub fn parse_meteora_dlmm(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<PoolState> {
    const MIN_LEN: usize = 216;
    if data.len() < MIN_LEN {
        return None;
    }

    let active_id = i32::from_le_bytes(data[76..80].try_into().ok()?);
    // Pitfall #17: active_id max is ~443636, values like 8388608 are garbage
    if active_id.unsigned_abs() > 500_000 {
        return None;
    }
    let bin_step = u16::from_le_bytes(data[80..82].try_into().ok()?);
    let mint_x = Pubkey::new_from_array(data[88..120].try_into().ok()?);
    let mint_y = Pubkey::new_from_array(data[120..152].try_into().ok()?);

    // Synthetic reserves: price = (1 + bin_step/10000)^active_id
    // Use integer approximation suitable for route discovery (not simulation).
    // We represent price as a ratio with a fixed denominator of 1_000_000.
    let bin_step_f = bin_step as f64 / 10_000.0;
    let price = (1.0 + bin_step_f).powi(active_id);
    let synthetic_reserve_a: u64 = 1_000_000_000; // 1 token reference amount
    let synthetic_reserve_b: u64 = ((synthetic_reserve_a as f64) * price) as u64;

    Some(PoolState {
        address: *pool_address,
        dex_type: DexType::MeteoraDlmm,
        token_a_mint: mint_x,
        token_b_mint: mint_y,
        token_a_reserve: synthetic_reserve_a,
        token_b_reserve: synthetic_reserve_b,
        fee_bps: DexType::MeteoraDlmm.base_fee_bps(),
        current_tick: Some(active_id),
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: slot,
        extra: {
            let spl_token = addresses::SPL_TOKEN;
            let token_2022 = addresses::TOKEN_2022;
            // token_mint_x_program_flag at offset 878, y at 879 (0=SPL Token, 1=Token-2022)
            let prog_x = if data.len() > 878 && data[878] == 1 { token_2022 } else { spl_token };
            let prog_y = if data.len() > 879 && data[879] == 1 { token_2022 } else { spl_token };
            PoolExtra {
                vault_a: Some(Pubkey::new_from_array(data[152..184].try_into().ok()?)),
                vault_b: Some(Pubkey::new_from_array(data[184..216].try_into().ok()?)),
                token_program_a: Some(prog_x),
                token_program_b: Some(prog_y),
                ..Default::default()
            }
        },
        best_bid_price: None,
        best_ask_price: None,
    })
}
