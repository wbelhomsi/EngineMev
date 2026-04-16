use solana_sdk::pubkey::Pubkey;
use crate::router::pool::{DexType, PoolExtra, PoolState};

/// Parse a PumpSwap AMM pool account.
///
/// Pool layout (243-301 bytes):
///   0..8    discriminator [0xf1, 0x9a, 0x6d, 0x04, 0x11, 0xb1, 0x6d, 0xbc]
///   8       pool_bump (u8)
///   9..11   index (u16)
///   11..43  creator (Pubkey)
///   43..75  base_mint (Pubkey)
///   75..107 quote_mint (Pubkey) — always wSOL
///  107..139 lp_mint (Pubkey)
///  139..171 pool_base_token_account (base vault)
///  171..203 pool_quote_token_account (quote vault)
///  203..211 lp_supply (u64)
///  211..243 coin_creator (Pubkey)
///  243      is_mayhem_mode (optional u8)
///  244      is_cashback_coin (optional u8)
///
/// Returns (PoolState, (base_vault, quote_vault)) for lazy vault fetch.
pub fn parse_pumpswap(
    pool_address: &Pubkey,
    data: &[u8],
    slot: u64,
) -> Option<(PoolState, (Pubkey, Pubkey))> {
    const MIN_LEN: usize = 243;
    const DISCRIMINATOR: [u8; 8] = [0xf1, 0x9a, 0x6d, 0x04, 0x11, 0xb1, 0x6d, 0xbc];

    if data.len() < MIN_LEN {
        return None;
    }
    if data[0..8] != DISCRIMINATOR {
        return None;
    }

    let base_mint = Pubkey::new_from_array(data[43..75].try_into().ok()?);
    let quote_mint = Pubkey::new_from_array(data[75..107].try_into().ok()?);
    let base_vault = Pubkey::new_from_array(data[139..171].try_into().ok()?);
    let quote_vault = Pubkey::new_from_array(data[171..203].try_into().ok()?);
    let coin_creator = Pubkey::new_from_array(data[211..243].try_into().ok()?);
    let is_mayhem_mode = if data.len() > 243 { data[243] != 0 } else { false };
    let is_cashback_coin = if data.len() > 244 { data[244] != 0 } else { false };

    let pool = PoolState {
        address: *pool_address,
        dex_type: DexType::PumpSwap,
        token_a_mint: base_mint,
        token_b_mint: quote_mint,
        token_a_reserve: 0, // populated after vault fetch
        token_b_reserve: 0,
        fee_bps: 125, // conservative worst-case (tiered 30-125 bps)
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: slot,
        extra: PoolExtra {
            vault_a: Some(base_vault),
            vault_b: Some(quote_vault),
            coin_creator: Some(coin_creator),
            is_mayhem_mode: Some(is_mayhem_mode),
            is_cashback_coin: Some(is_cashback_coin),
            token_program_b: Some(crate::addresses::SPL_TOKEN), // quote is always wSOL
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    };

    Some((pool, (base_vault, quote_vault)))
}
