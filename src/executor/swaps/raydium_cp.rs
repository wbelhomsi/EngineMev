use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

use crate::addresses;
use crate::router::pool::PoolState;

use super::derive_ata_with_program;

/// Build a Raydium CP-Swap instruction with the full 13-account layout.
///
/// Program: CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C
/// Discriminator: [0x8f, 0xbe, 0x5a, 0xda, 0xc4, 0x1e, 0x33, 0xde] (swap_base_in)
pub fn build_raydium_cp_swap_ix(
    signer: &Pubkey,
    pool: &PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let extra = &pool.extra;
    let vault_a = extra.vault_a?;
    let vault_b = extra.vault_b?;
    let amm_config = extra.config?;
    let token_prog_a = extra.token_program_a?;
    let token_prog_b = extra.token_program_b?;

    let cp_program = addresses::RAYDIUM_CP;
    let (authority, _) = Pubkey::find_program_address(
        &[b"vault_and_lp_mint_auth_seed"], &cp_program,
    );
    let (observation, _) = Pubkey::find_program_address(
        &[b"observation", pool.address.as_ref()], &cp_program,
    );

    let a_to_b = input_mint == pool.token_a_mint;
    let (input_vault, output_vault) = if a_to_b { (vault_a, vault_b) } else { (vault_b, vault_a) };
    let (input_token_prog, output_token_prog) = if a_to_b { (token_prog_a, token_prog_b) } else { (token_prog_b, token_prog_a) };
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };

    // Use derive_ata_with_program — Raydium CP supports Token-2022 per side
    let user_input_ata = derive_ata_with_program(signer, &input_mint, &input_token_prog);
    let user_output_ata = derive_ata_with_program(signer, &output_mint, &output_token_prog);

    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&[0x8f, 0xbe, 0x5a, 0xda, 0xc4, 0x1e, 0x33, 0xde]);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());

    let accounts = vec![
        AccountMeta::new(*signer, true),
        AccountMeta::new_readonly(authority, false),
        AccountMeta::new_readonly(amm_config, false),
        AccountMeta::new(pool.address, false),
        AccountMeta::new(user_input_ata, false),
        AccountMeta::new(user_output_ata, false),
        AccountMeta::new(input_vault, false),
        AccountMeta::new(output_vault, false),
        AccountMeta::new_readonly(input_token_prog, false),
        AccountMeta::new_readonly(output_token_prog, false),
        AccountMeta::new_readonly(input_mint, false),
        AccountMeta::new_readonly(output_mint, false),
        AccountMeta::new(observation, false),
    ];

    Some(Instruction { program_id: cp_program, accounts, data })
}
