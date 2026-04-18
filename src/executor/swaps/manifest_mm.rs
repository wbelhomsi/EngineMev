//! Manifest CLOB market-making instruction builders.
//!
//! Manifest uses **custom u8 opcodes** (`#[repr(u8)]` on `ManifestInstruction`) —
//! NOT Anchor discriminators. Params are Borsh-serialized and appended
//! after the opcode byte.
//!
//! Key opcodes used by an MM bot:
//!   1 = ClaimSeat     — one-time per (market, trader)
//!   2 = Deposit       — fund seat with base or quote inventory
//!   3 = Withdraw      — pull inventory back out of seat
//!   6 = BatchUpdate   — place/cancel resting orders atomically
//!
//! A seat is allocated inside the market account (hypertree node), not
//! as a separate PDA. Inventory lives in per-market vault PDAs:
//!   `[b"vault", market, mint]` → SPL Token account owned by program.
//!
//! Orders are identified by `order_sequence_number: u64` assigned by the
//! program on placement. Cancels reference this sequence number.
//!
//! References:
//!   - CKS-Systems/manifest (programs/manifest/src/program/instruction.rs)
//!   - Manifest Orderbook Manifesto PDF

use borsh::{BorshDeserialize, BorshSerialize};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

use crate::addresses::{MANIFEST, SPL_TOKEN};

/// Manifest instruction opcodes (first byte of instruction data).
#[repr(u8)]
#[allow(dead_code)]
pub enum ManifestOpcode {
    CreateMarket = 0,
    ClaimSeat = 1,
    Deposit = 2,
    Withdraw = 3,
    Swap = 4,
    Expand = 5,
    BatchUpdate = 6,
    GlobalCreate = 7,
    SwapV2 = 13,
}

/// DataIndex type used by Manifest's hypertree for seat/order refs.
pub type DataIndex = u32;

/// Order type constants for `PlaceOrderParams::order_type` (u8 wire field).
/// Kept as a module of u8 constants (rather than an enum) to sidestep borsh
/// enum-derive quirks; the on-wire discriminant is what the program sees.
pub mod order_type {
    pub const LIMIT: u8 = 0;
    pub const IMMEDIATE_OR_CANCEL: u8 = 1;
    /// Rejects rather than crossing the book. Use for pure resting MM quotes.
    pub const POST_ONLY: u8 = 2;
    pub const GLOBAL: u8 = 3;
    pub const REVERSE: u8 = 4;
    pub const REVERSE_TIGHT: u8 = 5;
}

/// Place-order params (per order).
///
/// Price is stored as `price = mantissa * 10^exponent` where both are
/// signed-ish fields. Units: quote atoms per base atom.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct PlaceOrderParams {
    pub base_atoms: u64,
    pub price_mantissa: u32,
    pub price_exponent: i8,
    pub is_bid: bool,
    /// 0 = never expires.
    pub last_valid_slot: u32,
    /// See `order_type` constants. Wire value is u8; program rejects invalid.
    pub order_type: u8,
}

/// Cancel-order params. Reference by sequence number.
///
/// `order_index_hint` is a fast-path: when `Some`, the program jumps
/// directly to that hypertree node. When `None`, the program walks
/// the tree to find the matching sequence. Safe to always pass `None`
/// for simplicity at the cost of a few extra CU per cancel.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct CancelOrderParams {
    pub order_sequence_number: u64,
    pub order_index_hint: Option<DataIndex>,
}

/// BatchUpdate instruction params. Cancels are applied first, then places.
/// Both lists can be empty (no-op is legal but pointless).
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct BatchUpdateParams {
    /// Fast-path for locating the trader's seat. `None` = walk the tree.
    pub trader_index_hint: Option<DataIndex>,
    pub cancels: Vec<CancelOrderParams>,
    pub orders: Vec<PlaceOrderParams>,
}

/// Deposit/Withdraw params. Shared layout.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct DepositParams {
    pub amount_atoms: u64,
    pub trader_index_hint: Option<DataIndex>,
}
pub type WithdrawParams = DepositParams;

/// PDA helper: vault address for (market, mint).
pub fn get_vault_address(market: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"vault", market.as_ref(), mint.as_ref()],
        &MANIFEST,
    )
    .0
}

// ---------------------------------------------------------------------------
// Instruction 1: ClaimSeat — one-time per (market, trader).
// ---------------------------------------------------------------------------
/// Build a `ClaimSeat` instruction. Must be called once per (market, trader)
/// before any deposits or order placements.
pub fn build_claim_seat_ix(payer: &Pubkey, market: &Pubkey) -> Instruction {
    let accounts = vec![
        AccountMeta::new(*payer, true),  // 0: payer, writable, signer
        AccountMeta::new(*market, false), // 1: market, writable
        AccountMeta::new_readonly(solana_system_interface::program::id(), false), // 2: system program
    ];
    // No params for ClaimSeat.
    Instruction {
        program_id: MANIFEST,
        accounts,
        data: vec![ManifestOpcode::ClaimSeat as u8],
    }
}

// ---------------------------------------------------------------------------
// Instruction 2: Deposit — fund seat.
// ---------------------------------------------------------------------------
/// Build a `Deposit` instruction — moves tokens from the trader's ATA into
/// the market's per-mint vault PDA, crediting the trader's seat.
pub fn build_deposit_ix(
    payer: &Pubkey,
    market: &Pubkey,
    mint: &Pubkey,
    trader_token_account: &Pubkey,
    amount_atoms: u64,
) -> Instruction {
    let vault = get_vault_address(market, mint);
    let accounts = vec![
        AccountMeta::new_readonly(*payer, true),          // 0: payer, readonly signer
        AccountMeta::new(*market, false),                  // 1: market, writable
        AccountMeta::new(*trader_token_account, false),    // 2: trader ATA, writable
        AccountMeta::new(vault, false),                    // 3: vault PDA, writable
        AccountMeta::new_readonly(SPL_TOKEN, false),       // 4: token program
        AccountMeta::new_readonly(*mint, false),           // 5: mint
    ];
    let params = DepositParams {
        amount_atoms,
        trader_index_hint: None,
    };
    let mut data = vec![ManifestOpcode::Deposit as u8];
    data.extend(borsh::to_vec(&params).expect("borsh serialize DepositParams"));
    Instruction {
        program_id: MANIFEST,
        accounts,
        data,
    }
}

// ---------------------------------------------------------------------------
// Instruction 3: Withdraw — pull inventory back out.
// ---------------------------------------------------------------------------
pub fn build_withdraw_ix(
    payer: &Pubkey,
    market: &Pubkey,
    mint: &Pubkey,
    trader_token_account: &Pubkey,
    amount_atoms: u64,
) -> Instruction {
    let vault = get_vault_address(market, mint);
    let accounts = vec![
        AccountMeta::new_readonly(*payer, true),
        AccountMeta::new(*market, false),
        AccountMeta::new(*trader_token_account, false),
        AccountMeta::new(vault, false),
        AccountMeta::new_readonly(SPL_TOKEN, false),
        AccountMeta::new_readonly(*mint, false),
    ];
    let params = WithdrawParams {
        amount_atoms,
        trader_index_hint: None,
    };
    let mut data = vec![ManifestOpcode::Withdraw as u8];
    data.extend(borsh::to_vec(&params).expect("borsh serialize WithdrawParams"));
    Instruction {
        program_id: MANIFEST,
        accounts,
        data,
    }
}

// ---------------------------------------------------------------------------
// Instruction 6: BatchUpdate — cancel + place atomically.
// ---------------------------------------------------------------------------
/// Build a BatchUpdate instruction that cancels zero or more resting orders
/// and places zero or more new orders atomically.
///
/// Account layout:
///   0: payer (writable, signer)
///   1: market (writable)
///   2: system program (readonly)
///
/// Mint / global / vault blocks are only required when the placed orders
/// would cross the book (i.e. match against global liquidity or cause
/// settlement). For pure resting PostOnly orders that stay in the trader's
/// book side, these are not needed — this keeps the IX small.
pub fn build_batch_update_ix(
    payer: &Pubkey,
    market: &Pubkey,
    cancels: Vec<CancelOrderParams>,
    orders: Vec<PlaceOrderParams>,
) -> Instruction {
    let accounts = vec![
        AccountMeta::new(*payer, true),
        AccountMeta::new(*market, false),
        AccountMeta::new_readonly(solana_system_interface::program::id(), false),
    ];
    let params = BatchUpdateParams {
        trader_index_hint: None,
        cancels,
        orders,
    };
    let mut data = vec![ManifestOpcode::BatchUpdate as u8];
    data.extend(borsh::to_vec(&params).expect("borsh serialize BatchUpdateParams"));
    Instruction {
        program_id: MANIFEST,
        accounts,
        data,
    }
}

/// Convenience: place a single PostOnly limit order (no cancellations).
pub fn build_place_post_only_ix(
    payer: &Pubkey,
    market: &Pubkey,
    base_atoms: u64,
    price_mantissa: u32,
    price_exponent: i8,
    is_bid: bool,
) -> Instruction {
    build_batch_update_ix(
        payer,
        market,
        vec![],
        vec![PlaceOrderParams {
            base_atoms,
            price_mantissa,
            price_exponent,
            is_bid,
            last_valid_slot: 0,
            order_type: order_type::POST_ONLY,
        }],
    )
}

/// Convenience: cancel a single order by sequence number.
pub fn build_cancel_ix(
    payer: &Pubkey,
    market: &Pubkey,
    order_sequence_number: u64,
) -> Instruction {
    build_batch_update_ix(
        payer,
        market,
        vec![CancelOrderParams {
            order_sequence_number,
            order_index_hint: None,
        }],
        vec![],
    )
}

/// Convenience: cancel-all-then-repost in one atomic IX. Standard quote refresh.
pub fn build_requote_ix(
    payer: &Pubkey,
    market: &Pubkey,
    cancel_seq_numbers: &[u64],
    new_orders: Vec<PlaceOrderParams>,
) -> Instruction {
    let cancels = cancel_seq_numbers
        .iter()
        .map(|&seq| CancelOrderParams {
            order_sequence_number: seq,
            order_index_hint: None,
        })
        .collect();
    build_batch_update_ix(payer, market, cancels, new_orders)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claim_seat_ix_shape() {
        let payer = Pubkey::new_unique();
        let market = Pubkey::new_unique();
        let ix = build_claim_seat_ix(&payer, &market);
        assert_eq!(ix.program_id, MANIFEST);
        assert_eq!(ix.data, vec![ManifestOpcode::ClaimSeat as u8]);
        assert_eq!(ix.accounts.len(), 3);
        assert_eq!(ix.accounts[0].pubkey, payer);
        assert!(ix.accounts[0].is_writable);
        assert!(ix.accounts[0].is_signer);
        assert_eq!(ix.accounts[1].pubkey, market);
        assert!(ix.accounts[1].is_writable);
    }

    #[test]
    fn test_deposit_ix_shape() {
        let payer = Pubkey::new_unique();
        let market = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let ata = Pubkey::new_unique();
        let ix = build_deposit_ix(&payer, &market, &mint, &ata, 1_000_000);

        assert_eq!(ix.program_id, MANIFEST);
        assert_eq!(ix.data[0], ManifestOpcode::Deposit as u8);
        assert_eq!(ix.accounts.len(), 6);

        // Sanity: vault is deterministic
        let vault = get_vault_address(&market, &mint);
        assert_eq!(ix.accounts[3].pubkey, vault);
    }

    #[test]
    fn test_batch_update_place_only() {
        let payer = Pubkey::new_unique();
        let market = Pubkey::new_unique();
        let ix = build_place_post_only_ix(&payer, &market, 100, 42, -2, true);
        assert_eq!(ix.program_id, MANIFEST);
        assert_eq!(ix.data[0], ManifestOpcode::BatchUpdate as u8);
        assert_eq!(ix.accounts.len(), 3); // no mint/vault blocks for post-only

        // Round-trip: deserialize the borsh payload and confirm fields.
        let params: BatchUpdateParams =
            borsh::from_slice(&ix.data[1..]).expect("deserialize batch update");
        assert!(params.cancels.is_empty());
        assert_eq!(params.orders.len(), 1);
        assert_eq!(params.orders[0].base_atoms, 100);
        assert_eq!(params.orders[0].price_mantissa, 42);
        assert_eq!(params.orders[0].price_exponent, -2);
        assert!(params.orders[0].is_bid);
        assert_eq!(params.orders[0].order_type, order_type::POST_ONLY);
    }

    #[test]
    fn test_batch_update_cancel_only() {
        let payer = Pubkey::new_unique();
        let market = Pubkey::new_unique();
        let ix = build_cancel_ix(&payer, &market, 12345);
        let params: BatchUpdateParams =
            borsh::from_slice(&ix.data[1..]).expect("deserialize");
        assert!(params.orders.is_empty());
        assert_eq!(params.cancels.len(), 1);
        assert_eq!(params.cancels[0].order_sequence_number, 12345);
        assert!(params.cancels[0].order_index_hint.is_none());
    }

    #[test]
    fn test_requote_cancels_then_places() {
        let payer = Pubkey::new_unique();
        let market = Pubkey::new_unique();
        let new_orders = vec![
            PlaceOrderParams {
                base_atoms: 100,
                price_mantissa: 10,
                price_exponent: -1,
                is_bid: true,
                last_valid_slot: 0,
                order_type: order_type::POST_ONLY,
            },
            PlaceOrderParams {
                base_atoms: 100,
                price_mantissa: 12,
                price_exponent: -1,
                is_bid: false,
                last_valid_slot: 0,
                order_type: order_type::POST_ONLY,
            },
        ];
        let ix = build_requote_ix(&payer, &market, &[111, 222], new_orders);
        let params: BatchUpdateParams =
            borsh::from_slice(&ix.data[1..]).expect("deserialize");
        assert_eq!(params.cancels.len(), 2);
        assert_eq!(params.cancels[0].order_sequence_number, 111);
        assert_eq!(params.cancels[1].order_sequence_number, 222);
        assert_eq!(params.orders.len(), 2);
        assert!(params.orders[0].is_bid);
        assert!(!params.orders[1].is_bid);
    }

    #[test]
    fn test_vault_pda_deterministic() {
        let market = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let a = get_vault_address(&market, &mint);
        let b = get_vault_address(&market, &mint);
        assert_eq!(a, b);
    }
}
