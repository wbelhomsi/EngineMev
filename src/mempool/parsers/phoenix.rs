use solana_sdk::pubkey::Pubkey;
use crate::router::pool::{DexType, PoolExtra, PoolState};

/// Walk a Phoenix sokoban RedBlackTree to find the best (minimum/leftmost) order.
/// Returns (Some(price_in_ticks), depth_base_atoms) or (None, 0) if tree is empty.
///
/// Tree layout at `tree_start`:
///   +0:  root (u32, 1-based index; 0 = SENTINEL = empty)
///   +4:  12 bytes padding
///   +16: NodeAllocator header: size (u64), bump_index (u32), free_list_head (u32)
///   +32: nodes array, each node 64 bytes
///
/// Node layout (64 bytes):
///   +0:  left (u32), right (u32), parent (u32), color (u32) — 16 bytes registers
///   +16: price_in_ticks (u64)
///   +24: order_sequence_number (u64)
///   +32: trader_index (u64)
///   +40: num_base_lots (u64)
///   +48: last_valid_slot (u64)
///   +56: last_valid_unix_timestamp (u64)
fn phoenix_tree_best(data: &[u8], tree_start: usize, base_lot_size: u64) -> (Option<u64>, u64) {
    if data.len() < tree_start + 32 {
        return (None, 0);
    }

    let root = u32::from_le_bytes(
        data[tree_start..tree_start + 4].try_into().unwrap_or([0; 4]),
    );
    if root == 0 {
        return (None, 0);
    }

    let nodes_start = tree_start + 32;

    // Follow left children from root to find minimum (leftmost) node
    let mut current = root;
    for _ in 0..1000 {
        // safety limit against corrupt data
        if current == 0 {
            return (None, 0);
        }
        let node_off = nodes_start + (current as usize - 1) * 64;
        if node_off + 64 > data.len() {
            return (None, 0);
        }

        let left =
            u32::from_le_bytes(data[node_off..node_off + 4].try_into().unwrap_or([0; 4]));
        if left == 0 {
            // Found the minimum node
            let price_in_ticks = u64::from_le_bytes(
                data[node_off + 16..node_off + 24]
                    .try_into()
                    .unwrap_or([0; 8]),
            );
            let num_base_lots = u64::from_le_bytes(
                data[node_off + 40..node_off + 48]
                    .try_into()
                    .unwrap_or([0; 8]),
            );
            return (
                Some(price_in_ticks),
                num_base_lots.saturating_mul(base_lot_size),
            );
        }
        current = left;
    }
    (None, 0)
}

/// Parse a Phoenix V1 market account (header >= 624 bytes, variable total size).
///
/// Layout (byte offsets from MarketHeader):
///   16  bids_size (u64) — number of nodes allocated for bids tree
///   24  asks_size (u64) — number of nodes allocated for asks tree
///   48  base_mint (Pubkey, 32)
///   80  base_vault (Pubkey, 32)
///   136 base_lot_size (u64, 8)
///   152 quote_mint (Pubkey, 32)
///   184 quote_vault (Pubkey, 32)
///   240 quote_lot_size (u64, 8)
///   248 tick_size_in_quote_atoms_per_base_unit (u64, 8)
///
/// FIFOMarket starts at offset 624:
///   +280 taker_fee_bps (u64)
///   +304 bids RedBlackTree starts (offset 928 absolute)
///   asks tree starts at 928 + 32 + bids_size * 64
pub fn parse_phoenix_market(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<PoolState> {
    const HEADER_LEN: usize = 624;
    if data.len() < HEADER_LEN {
        return None;
    }

    let base_mint = Pubkey::new_from_array(data[48..80].try_into().ok()?);
    let quote_mint = Pubkey::new_from_array(data[152..184].try_into().ok()?);
    let base_vault = Pubkey::new_from_array(data[80..112].try_into().ok()?);
    let quote_vault = Pubkey::new_from_array(data[184..216].try_into().ok()?);
    let base_lot_size = u64::from_le_bytes(data[136..144].try_into().ok()?);
    let quote_lot_size = u64::from_le_bytes(data[240..248].try_into().ok()?);

    if base_lot_size == 0 || quote_lot_size == 0 {
        return None;
    }
    if base_mint == Pubkey::default() || quote_mint == Pubkey::default() {
        return None;
    }

    // Read tick_size for price conversion: price = price_in_ticks * tick_size
    let tick_size = u64::from_le_bytes(data[248..256].try_into().ok()?);

    // Read taker fee from FIFOMarket header (offset 624 + 280 = 904)
    let fee_bps = if data.len() > 624 + 288 {
        u64::from_le_bytes(data[624 + 280..624 + 288].try_into().ok()?)
    } else {
        DexType::Phoenix.base_fee_bps()
    };

    // Extract top-of-book from bids and asks RedBlackTrees
    let bids_size = u64::from_le_bytes(data[16..24].try_into().ok()?) as usize;
    let asks_size = u64::from_le_bytes(data[24..32].try_into().ok()?) as usize;

    // Bids tree starts at offset 928
    const BIDS_TREE_START: usize = 928;
    let (best_bid_ticks, bid_depth) = phoenix_tree_best(data, BIDS_TREE_START, base_lot_size);

    // Asks tree starts after bids: 928 + 32 (header) + bids_size * 64 (nodes)
    let asks_tree_start = BIDS_TREE_START + 32 + bids_size.checked_mul(64)?;
    let (best_ask_ticks, ask_depth) = phoenix_tree_best(data, asks_tree_start, base_lot_size);
    let _ = asks_size; // used implicitly via tree root/nodes

    // Convert price_in_ticks to quote atoms per base unit
    let best_bid_price = best_bid_ticks.map(|ticks| (ticks as u128) * (tick_size as u128));
    let best_ask_price = best_ask_ticks.map(|ticks| (ticks as u128) * (tick_size as u128));

    Some(PoolState {
        address: *pool_address,
        dex_type: DexType::Phoenix,
        token_a_mint: base_mint,
        token_b_mint: quote_mint,
        token_a_reserve: bid_depth,
        token_b_reserve: ask_depth,
        fee_bps,
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

/// Try to parse a variable-size account as an orderbook DEX market.
pub fn try_parse_orderbook(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<PoolState> {
    if data.len() >= 624 {
        if let Some(pool) = parse_phoenix_market(pool_address, data, slot) {
            return Some(pool);
        }
    }
    if data.len() >= 256 {
        if let Some(pool) = super::manifest::parse_manifest_market(pool_address, data, slot) {
            return Some(pool);
        }
    }
    None
}
