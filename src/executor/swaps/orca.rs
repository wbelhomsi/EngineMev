use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

use crate::addresses;
use crate::router::pool::PoolState;

use super::{derive_ata_with_program, floor_div};

/// Build an Orca Whirlpool swap_v2 instruction with full account layout.
///
/// Program: whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc
/// Discriminator: [0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62] (swap_v2)
/// Accounts: 15 (token_program_a, token_program_b, memo_program, token_authority, whirlpool,
///           token_mint_a, token_mint_b, ata_a, vault_a, ata_b, vault_b,
///           tick_array_0, tick_array_1, tick_array_2, oracle)
pub fn build_orca_whirlpool_swap_ix(
    signer: &Pubkey,
    pool: &PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let extra = &pool.extra;
    let vault_a = extra.vault_a?;
    let vault_b = extra.vault_b?;
    let tick_spacing = extra.tick_spacing?;

    let whirlpool_program = addresses::ORCA_WHIRLPOOL;
    let token_program_a = extra.token_program_a.unwrap_or(addresses::SPL_TOKEN);
    let token_program_b = extra.token_program_b.unwrap_or(addresses::SPL_TOKEN);
    let memo_program = addresses::MEMO;

    let a_to_b = input_mint == pool.token_a_mint;
    let tick_current = pool.current_tick.unwrap_or(0);

    // Oracle PDA
    let (oracle, _) = Pubkey::find_program_address(
        &[b"oracle", pool.address.as_ref()], &whirlpool_program,
    );

    // Tick array PDAs (3 arrays, string-encoded start index)
    let ticks_in_array: i32 = 88 * tick_spacing as i32;
    let start_base = floor_div(tick_current, ticks_in_array) * ticks_in_array;

    let offsets: [i32; 3] = if a_to_b {
        [0, -1, -2]
    } else if tick_current + tick_spacing as i32 >= start_base + ticks_in_array {
        [1, 2, 3]
    } else {
        [0, 1, 2]
    };

    let tick_arrays: Vec<Pubkey> = offsets.iter().map(|&o| {
        let start = start_base + o * ticks_in_array;
        Pubkey::find_program_address(
            &[b"tick_array", pool.address.as_ref(), start.to_string().as_bytes()],
            &whirlpool_program,
        ).0
    }).collect();

    // sqrt_price_limit
    let sqrt_price_limit: u128 = if a_to_b { 4295048016u128 } else { 79226673515401279992447579055u128 };

    // User token accounts
    let user_ata_a = derive_ata_with_program(signer, &pool.token_a_mint, &token_program_a);
    let user_ata_b = derive_ata_with_program(signer, &pool.token_b_mint, &token_program_b);

    // Discriminator: swap_v2 [0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62]
    let mut data = Vec::with_capacity(43);
    data.extend_from_slice(&[0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62]);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    data.extend_from_slice(&sqrt_price_limit.to_le_bytes());
    data.push(1u8); // is_exact_in = true
    data.push(if a_to_b { 1u8 } else { 0u8 }); // a_to_b
    data.push(0u8); // remaining_accounts_info = None

    // SwapV2 account layout (15 accounts):
    let accounts = vec![
        AccountMeta::new_readonly(token_program_a, false),   // 0: token_program_a
        AccountMeta::new_readonly(token_program_b, false),   // 1: token_program_b
        AccountMeta::new_readonly(memo_program, false),    // 2: memo_program
        AccountMeta::new(*signer, true),                   // 3: token_authority (signer)
        AccountMeta::new(pool.address, false),             // 4: whirlpool
        AccountMeta::new_readonly(pool.token_a_mint, false), // 5: token_mint_a
        AccountMeta::new_readonly(pool.token_b_mint, false), // 6: token_mint_b
        AccountMeta::new(user_ata_a, false),               // 7: token_owner_account_a
        AccountMeta::new(vault_a, false),                  // 8: token_vault_a
        AccountMeta::new(user_ata_b, false),               // 9: token_owner_account_b
        AccountMeta::new(vault_b, false),                  // 10: token_vault_b
        AccountMeta::new(tick_arrays[0], false),           // 11: tick_array_0
        AccountMeta::new(tick_arrays[1], false),           // 12: tick_array_1
        AccountMeta::new(tick_arrays[2], false),           // 13: tick_array_2
        AccountMeta::new(oracle, false),                   // 14: oracle
    ];

    Some(Instruction { program_id: whirlpool_program, accounts, data })
}
