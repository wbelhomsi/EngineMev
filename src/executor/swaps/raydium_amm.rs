use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

use crate::addresses;
use crate::router::pool::PoolState;

use super::derive_ata;

/// Build a Raydium AMM v4 SwapBaseInV2 instruction (8 accounts).
///
/// V2 removes all Serum/OpenBook dependencies. Only needs vault_a, vault_b,
/// and amm_nonce (for authority PDA derivation).
///
/// Discriminator: 16 (V1 was 9).
/// Data: [discriminator(1), amount_in(8), min_out(8)] = 17 bytes.
/// Accounts: SPL Token, amm_id, amm_authority, coin_vault, pc_vault,
///           user_source, user_dest, signer.
pub fn build_raydium_amm_swap_ix(
    signer: &Pubkey,
    pool: &PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let extra = &pool.extra;
    let vault_a = extra.vault_a?;
    let vault_b = extra.vault_b?;
    let nonce = extra.amm_nonce?;

    let amm_program = addresses::RAYDIUM_AMM;
    let amm_authority = Pubkey::create_program_address(
        &[&[nonce]],
        &amm_program,
    ).ok()?;

    let a_to_b = input_mint == pool.token_a_mint;
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };
    let user_source_ata = derive_ata(signer, &input_mint);
    let user_dest_ata = derive_ata(signer, &output_mint);

    // SwapBaseInV2 discriminator = 16
    let mut data = Vec::with_capacity(17);
    data.push(16u8);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());

    let accounts = vec![
        AccountMeta::new_readonly(addresses::SPL_TOKEN, false),   // [0] token_program
        AccountMeta::new(pool.address, false),                     // [1] amm_id
        AccountMeta::new_readonly(amm_authority, false),           // [2] amm_authority
        AccountMeta::new(vault_a, false),                          // [3] pool_coin_token_account
        AccountMeta::new(vault_b, false),                          // [4] pool_pc_token_account
        AccountMeta::new(user_source_ata, false),                  // [5] user_source
        AccountMeta::new(user_dest_ata, false),                    // [6] user_dest
        AccountMeta::new_readonly(*signer, true),                  // [7] signer
    ];

    Some(Instruction { program_id: amm_program, accounts, data })
}
