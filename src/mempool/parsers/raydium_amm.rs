use solana_sdk::pubkey::Pubkey;
use crate::addresses;
use crate::router::pool::{DexType, PoolExtra, PoolState};

/// Parse a Raydium AMM v4 pool account (752 bytes).
///
/// Returns (PoolState, (base_vault, quote_vault)). Reserves are set to 0 until
/// the caller fetches the vault SPL Token accounts and populates them.
///
/// Layout (byte offsets):
///   0   status (u64, first 8 bytes encode pool state; 6 = initialized)
///   336 base_vault (Pubkey, 32)
///   368 quote_vault (Pubkey, 32)
///   400 base_mint (Pubkey, 32)
///   432 quote_mint (Pubkey, 32)
pub fn parse_raydium_amm_v4(
    pool_address: &Pubkey,
    data: &[u8],
    slot: u64,
) -> Option<(PoolState, (Pubkey, Pubkey))> {
    const MIN_LEN: usize = 624; // need to read up to target_orders at offset 592+32
    if data.len() < MIN_LEN {
        return None;
    }

    let nonce = data[8]; // offset 8, u64 but only lowest byte used

    // Extract trade fee from pool state (more accurate than hardcoded 25 bps)
    let trade_fee_num = u64::from_le_bytes(data[144..152].try_into().ok()?);
    let trade_fee_den = u64::from_le_bytes(data[152..160].try_into().ok()?);
    let fee_bps = if trade_fee_den > 0 {
        trade_fee_num * 10000 / trade_fee_den
    } else {
        25 // fallback
    };

    let base_vault = Pubkey::new_from_array(data[336..368].try_into().ok()?);
    let quote_vault = Pubkey::new_from_array(data[368..400].try_into().ok()?);
    let base_mint = Pubkey::new_from_array(data[400..432].try_into().ok()?);
    let quote_mint = Pubkey::new_from_array(data[432..464].try_into().ok()?);
    let open_orders = Pubkey::new_from_array(data[496..528].try_into().ok()?);
    let market_id = Pubkey::new_from_array(data[528..560].try_into().ok()?);
    let market_program = Pubkey::new_from_array(data[560..592].try_into().ok()?);
    let target_orders = Pubkey::new_from_array(data[592..624].try_into().ok()?);

    let pool = PoolState {
        address: *pool_address,
        dex_type: DexType::RaydiumAmm,
        token_a_mint: base_mint,
        token_b_mint: quote_mint,
        token_a_reserve: 0, // populated after vault fetch
        token_b_reserve: 0,
        fee_bps,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: slot,
        extra: {
            let spl_token = addresses::SPL_TOKEN;
            PoolExtra {
                vault_a: Some(base_vault),
                vault_b: Some(quote_vault),
                token_program_a: Some(spl_token),
                token_program_b: Some(spl_token),
                open_orders: Some(open_orders),
                market: Some(market_id),
                market_program: Some(market_program),
                target_orders: Some(target_orders),
                amm_nonce: Some(nonce),
                ..Default::default()
            }
        },
        best_bid_price: None,
        best_ask_price: None,
    };

    Some((pool, (base_vault, quote_vault)))
}
