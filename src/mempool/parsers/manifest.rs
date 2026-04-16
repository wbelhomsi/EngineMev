use solana_sdk::pubkey::Pubkey;
use crate::router::pool::{DexType, PoolExtra, PoolState};

/// Read a Manifest RestingOrder at the given DataIndex (byte offset into dynamic section).
/// Returns (price_d18: Option<u128>, num_base_atoms: u64).
///
/// DataIndex is a byte offset; absolute position = 256 (MarketFixed header) + index.
/// u32::MAX (0xFFFFFFFF) is the sentinel for an empty book side.
///
/// RBNode<RestingOrder> layout (80 bytes per node):
///   +0:  left (u32), right (u32), parent (u32), color+type+pad — 16 bytes
///   +16: price (u128, LE) — QuoteAtomsPerBaseAtom, D18 fixed-point (scaled by 10^18)
///   +32: num_base_atoms (u64)
///   +40: sequence_number (u64)
///   +48: trader_index (u32)
///   ...
fn manifest_read_order(data: &[u8], index: u32) -> (Option<u128>, u64) {
    if index == u32::MAX {
        return (None, 0);
    }
    let abs_offset = 256 + index as usize;
    // Need at least up to +40 (price u128 at +16..+32, num_base_atoms u64 at +32..+40)
    if abs_offset + 40 > data.len() {
        return (None, 0);
    }

    let price = u128::from_le_bytes(
        data[abs_offset + 16..abs_offset + 32]
            .try_into()
            .unwrap_or([0; 16]),
    );
    let num_base_atoms = u64::from_le_bytes(
        data[abs_offset + 32..abs_offset + 40]
            .try_into()
            .unwrap_or([0; 8]),
    );

    if price == 0 {
        return (None, 0);
    }
    (Some(price), num_base_atoms)
}

/// Parse a Manifest market account (fixed header = 256 bytes, variable total size).
///
/// Layout (byte offsets from MarketFixed):
///   16  base_mint (Pubkey, 32)
///   48  quote_mint (Pubkey, 32)
///   80  base_vault (Pubkey, 32)
///   112 quote_vault (Pubkey, 32)
///   160 bids_best_index (u32) — DataIndex (byte offset into dynamic section)
///   168 asks_best_index (u32)
///
/// Prices are D18 fixed-point (scaled by 10^18). The `get_orderbook_output()`
/// method in pool.rs handles the D18 division for Manifest pools.
pub fn parse_manifest_market(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<PoolState> {
    const HEADER_LEN: usize = 256;
    if data.len() < HEADER_LEN {
        return None;
    }

    let base_mint = Pubkey::new_from_array(data[16..48].try_into().ok()?);
    let quote_mint = Pubkey::new_from_array(data[48..80].try_into().ok()?);
    let base_vault = Pubkey::new_from_array(data[80..112].try_into().ok()?);
    let quote_vault = Pubkey::new_from_array(data[112..144].try_into().ok()?);

    if base_mint == Pubkey::default() || quote_mint == Pubkey::default() {
        return None;
    }

    // Extract top-of-book from best bid/ask indices
    let bids_best_idx = u32::from_le_bytes(data[160..164].try_into().ok()?);
    let asks_best_idx = u32::from_le_bytes(data[168..172].try_into().ok()?);

    let (best_bid_price, bid_depth) = manifest_read_order(data, bids_best_idx);
    let (best_ask_price, ask_depth) = manifest_read_order(data, asks_best_idx);

    Some(PoolState {
        address: *pool_address,
        dex_type: DexType::Manifest,
        token_a_mint: base_mint,
        token_b_mint: quote_mint,
        token_a_reserve: bid_depth,
        token_b_reserve: ask_depth,
        fee_bps: 0,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: slot,
        extra: PoolExtra {
            vault_a: Some(base_vault),
            vault_b: Some(quote_vault),
            ..Default::default()
        },
        best_bid_price,
        best_ask_price,
    })
}
