use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

use crate::addresses;
use crate::router::pool::PoolState;

use super::derive_ata;

/// Build a Phoenix swap instruction (ImmediateOrCancel order).
///
/// Program: PhoeNiXZ8ByJGLkxNfZRnkUfjvmuYqLR89jjFHGqdXY
/// Discriminant: 0x00 (Swap)
/// Accounts: 9 (phoenix_program, log_authority, market, trader, base_ata, quote_ata,
///           base_vault, quote_vault, token_program)
pub fn build_phoenix_swap_ix(
    signer: &Pubkey,
    pool: &PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let vault_a = pool.extra.vault_a?; // base vault
    let vault_b = pool.extra.vault_b?; // quote vault

    let phoenix_program = addresses::PHOENIX_V1;
    let token_program = addresses::SPL_TOKEN;

    let (log_authority, _) = Pubkey::find_program_address(&[b"log"], &phoenix_program);

    // token_a_mint = base, token_b_mint = quote
    let a_to_b = input_mint == pool.token_a_mint; // selling base = Ask side
    let side: u8 = if a_to_b { 0x01 } else { 0x00 }; // Ask=0x01, Bid=0x00

    let base_ata = derive_ata(signer, &pool.token_a_mint);
    let quote_ata = derive_ata(signer, &pool.token_b_mint);

    // Instruction data: discriminant + ImmediateOrCancel OrderPacket
    let mut data = Vec::with_capacity(44);
    data.push(0x00u8);             // instruction discriminant: Swap
    data.push(0x01u8);             // OrderPacket discriminant: ImmediateOrCancel
    data.push(side);               // side: 0x00=Bid, 0x01=Ask
    data.extend_from_slice(&0u64.to_le_bytes());              // price_in_ticks (0 = market)
    data.extend_from_slice(&amount_in.to_le_bytes());         // num_base_lots
    data.extend_from_slice(&0u64.to_le_bytes());              // num_quote_lots
    data.extend_from_slice(&minimum_amount_out.to_le_bytes()); // min_base_lots_to_fill
    data.extend_from_slice(&0u64.to_le_bytes());              // min_quote_lots_to_fill
    data.push(0x00u8);             // self_trade_behavior: Abort
    data.push(0x00u8);             // match_limit: None
    data.extend_from_slice(&0u128.to_le_bytes());             // client_order_id
    data.push(0x00u8);             // use_only_deposited_funds: false

    let accounts = vec![
        AccountMeta::new_readonly(phoenix_program, false), // 0: Phoenix program
        AccountMeta::new_readonly(log_authority, false),   // 1: Log authority PDA
        AccountMeta::new(pool.address, false),             // 2: Market (writable)
        AccountMeta::new(*signer, true),                   // 3: Trader/signer
        AccountMeta::new(base_ata, false),                 // 4: Trader base token account
        AccountMeta::new(quote_ata, false),                // 5: Trader quote token account
        AccountMeta::new(vault_a, false),                  // 6: Base vault
        AccountMeta::new(vault_b, false),                  // 7: Quote vault
        AccountMeta::new_readonly(token_program, false),   // 8: Token Program
    ];

    Some(Instruction { program_id: phoenix_program, accounts, data })
}
