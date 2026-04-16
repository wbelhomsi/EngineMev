use solana_sdk::pubkey::Pubkey;
use crate::router::pool::{DexType, PoolExtra, PoolState};

/// Parse a Raydium CP (constant-product) pool account (637 bytes).
///
/// Returns (PoolState, (vault_0, vault_1)). Reserves are set to 0 until
/// the caller fetches the vault SPL Token accounts.
///
/// Layout (byte offsets):
///   0   discriminator (8 bytes): [247, 237, 227, 245, 215, 195, 222, 70]
///   8   amm_config (Pubkey, 32)
///   72  token_0_vault (Pubkey, 32)
///   104 token_1_vault (Pubkey, 32)
///   168 token_0_mint (Pubkey, 32)
///   200 token_1_mint (Pubkey, 32)
///   232 token_0_program (Pubkey, 32)
///   264 token_1_program (Pubkey, 32)
pub fn parse_raydium_cp(
    pool_address: &Pubkey,
    data: &[u8],
    slot: u64,
) -> Option<(PoolState, (Pubkey, Pubkey))> {
    const MIN_LEN: usize = 296;
    if data.len() < MIN_LEN {
        return None;
    }

    let amm_config = Pubkey::new_from_array(data[8..40].try_into().ok()?);
    let vault_0 = Pubkey::new_from_array(data[72..104].try_into().ok()?);
    let vault_1 = Pubkey::new_from_array(data[104..136].try_into().ok()?);
    let mint_0 = Pubkey::new_from_array(data[168..200].try_into().ok()?);
    let mint_1 = Pubkey::new_from_array(data[200..232].try_into().ok()?);
    let token_0_program = Pubkey::new_from_array(data[232..264].try_into().ok()?);
    let token_1_program = Pubkey::new_from_array(data[264..296].try_into().ok()?);

    let pool = PoolState {
        address: *pool_address,
        dex_type: DexType::RaydiumCp,
        token_a_mint: mint_0,
        token_b_mint: mint_1,
        token_a_reserve: 0, // populated after vault fetch
        token_b_reserve: 0,
        fee_bps: DexType::RaydiumCp.base_fee_bps(),
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: slot,
        extra: PoolExtra {
            vault_a: Some(vault_0),
            vault_b: Some(vault_1),
            config: Some(amm_config),
            token_program_a: Some(token_0_program),
            token_program_b: Some(token_1_program),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    };

    Some((pool, (vault_0, vault_1)))
}
