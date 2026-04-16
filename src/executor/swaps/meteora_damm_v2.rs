use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

use crate::addresses;
use crate::router::pool::PoolState;

use super::derive_ata_with_program;

/// Build a Meteora DAMM v2 swap instruction with the full 12-account layout.
///
/// Program: cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG
/// Discriminator: [0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88] (swap)
pub fn build_damm_v2_swap_ix(
    signer: &Pubkey,
    pool: &PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let extra = &pool.extra;
    let vault_a = extra.vault_a?;
    let vault_b = extra.vault_b?;

    let damm_program = addresses::METEORA_DAMM_V2;
    let (pool_authority, _) = Pubkey::find_program_address(&[], &damm_program);
    let (event_authority, _) = Pubkey::find_program_address(&[b"__event_authority"], &damm_program);

    let a_to_b = input_mint == pool.token_a_mint;
    let (input_vault, output_vault) = if a_to_b { (vault_a, vault_b) } else { (vault_b, vault_a) };
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };

    let token_program_a = extra.token_program_a.unwrap_or(addresses::SPL_TOKEN);
    let token_program_b = extra.token_program_b.unwrap_or(addresses::SPL_TOKEN);

    let input_token_program = if a_to_b { token_program_a } else { token_program_b };
    let output_token_program = if a_to_b { token_program_b } else { token_program_a };

    let user_input_ata = derive_ata_with_program(signer, &input_mint, &input_token_program);
    let user_output_ata = derive_ata_with_program(signer, &output_mint, &output_token_program);

    // DAMM v2 swap2 has a single token_program account (account 8).
    // Use the input side's token program — the on-chain program handles both sides.
    let token_program = input_token_program;

    let mut data = Vec::with_capacity(25);
    data.extend_from_slice(&[0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88]);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    data.push(0u8); // swap_mode = 0 (ExactIn)

    let accounts = vec![
        AccountMeta::new(pool.address, false),
        AccountMeta::new_readonly(pool_authority, false),
        AccountMeta::new(input_vault, false),
        AccountMeta::new(output_vault, false),
        AccountMeta::new(user_input_ata, false),
        AccountMeta::new(user_output_ata, false),
        AccountMeta::new_readonly(input_mint, false),
        AccountMeta::new_readonly(output_mint, false),
        AccountMeta::new_readonly(token_program, false),
        AccountMeta::new_readonly(event_authority, false),
        AccountMeta::new_readonly(damm_program, false),
        AccountMeta::new(*signer, true),
    ];

    Some(Instruction { program_id: damm_program, accounts, data })
}
