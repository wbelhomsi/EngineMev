use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

use crate::addresses;
use crate::router::pool::PoolState;

use super::derive_ata;

/// Build a Manifest swap instruction.
///
/// Program: MNFSTqtC93rEfYHB6hF82sKdZpUDFWkViLByLd1k1Ms
/// Discriminant: 4 (Swap)
/// Accounts: 8 (payer, market, system_program, base_ata, quote_ata, base_vault, quote_vault,
///           token_program)
pub fn build_manifest_swap_ix(
    signer: &Pubkey,
    pool: &PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let vault_a = pool.extra.vault_a?; // base vault
    let vault_b = pool.extra.vault_b?; // quote vault

    let manifest_program = addresses::MANIFEST;
    let token_program = addresses::SPL_TOKEN;
    let system_program = solana_system_interface::program::id();

    // token_a_mint = base, token_b_mint = quote
    let is_base_in: u8 = if input_mint == pool.token_a_mint { 1 } else { 0 };

    let base_ata = derive_ata(signer, &pool.token_a_mint);
    let quote_ata = derive_ata(signer, &pool.token_b_mint);

    let mut data = Vec::with_capacity(19);
    data.push(4u8);                                           // Swap discriminant
    data.extend_from_slice(&amount_in.to_le_bytes());         // in_atoms
    data.extend_from_slice(&minimum_amount_out.to_le_bytes()); // out_atoms
    data.push(is_base_in);                                    // is_base_in
    data.push(1u8);                                           // is_exact_in = true

    let accounts = vec![
        AccountMeta::new(*signer, true),                   // 0: Payer/signer
        AccountMeta::new(pool.address, false),             // 1: Market
        AccountMeta::new_readonly(system_program, false),  // 2: System program
        AccountMeta::new(base_ata, false),                 // 3: Trader base token account
        AccountMeta::new(quote_ata, false),                // 4: Trader quote token account
        AccountMeta::new(vault_a, false),                  // 5: Base vault
        AccountMeta::new(vault_b, false),                  // 6: Quote vault
        AccountMeta::new_readonly(token_program, false),   // 7: Token program
    ];

    Some(Instruction { program_id: manifest_program, accounts, data })
}
