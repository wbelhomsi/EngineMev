use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

use crate::addresses;
use crate::router::pool::PoolState;

use super::{derive_ata_with_program, floor_div};

/// Build a Raydium CLMM swap_v2 instruction with full account layout.
///
/// Program: CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK
/// Discriminator: [0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62] (swap_v2)
/// Accounts: 17 (payer, amm_config, pool_state, input_ata, output_ata, input_vault, output_vault,
///           observation_state, token_program, token_2022, memo, input_mint, output_mint,
///           bitmap_extension, tick_array_0, tick_array_1, tick_array_2)
pub fn build_raydium_clmm_swap_ix(
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
    let amm_config = extra.config?;
    let observation_state = extra.observation?;

    let clmm_program = addresses::RAYDIUM_CLMM;
    let token_program = addresses::SPL_TOKEN;
    let token_2022_program = addresses::TOKEN_2022;
    let memo_program = addresses::MEMO;

    let a_to_b = input_mint == pool.token_a_mint;
    let tick_current = pool.current_tick.unwrap_or(0);

    let (input_vault, output_vault) = if a_to_b { (vault_a, vault_b) } else { (vault_b, vault_a) };
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };

    let input_token_program = if input_mint == pool.token_a_mint {
        extra.token_program_a.unwrap_or(addresses::SPL_TOKEN)
    } else {
        extra.token_program_b.unwrap_or(addresses::SPL_TOKEN)
    };
    let output_token_program = if output_mint == pool.token_a_mint {
        extra.token_program_a.unwrap_or(addresses::SPL_TOKEN)
    } else {
        extra.token_program_b.unwrap_or(addresses::SPL_TOKEN)
    };
    let user_input_ata = derive_ata_with_program(signer, &input_mint, &input_token_program);
    let user_output_ata = derive_ata_with_program(signer, &output_mint, &output_token_program);

    // Bitmap extension PDA
    let (bitmap_extension, _) = Pubkey::find_program_address(
        &[b"pool_tick_array_bitmap_extension", pool.address.as_ref()],
        &clmm_program,
    );

    // Tick array PDAs (3 arrays, i32 big-endian encoded)
    let ticks_in_array: i32 = 60 * tick_spacing as i32;
    let start_base = floor_div(tick_current, ticks_in_array) * ticks_in_array;

    let tick_offsets: [i32; 3] = if a_to_b {
        [0, -1, -2]
    } else {
        [0, 1, 2]
    };

    let tick_arrays: Vec<Pubkey> = tick_offsets.iter().map(|&o| {
        let start = start_base + o * ticks_in_array;
        Pubkey::find_program_address(
            &[b"tick_array", pool.address.as_ref(), &start.to_be_bytes()],
            &clmm_program,
        ).0
    }).collect();

    // Pass 0 — on-chain program substitutes correct MIN+1/MAX-1 and determines
    // direction from input vault mint. Eliminates wrong-constant failures.
    // Ref: raydium-clmm/programs/amm/src/instructions/swap_v2.rs lines 153-158
    let sqrt_price_limit: u128 = 0u128;

    // Discriminator: swap_v2
    let mut data = Vec::with_capacity(41);
    data.extend_from_slice(&[0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62]);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    data.extend_from_slice(&sqrt_price_limit.to_le_bytes());
    data.push(1u8); // is_exact_in = true

    let accounts = vec![
        AccountMeta::new(*signer, true),                       // 0: payer
        AccountMeta::new_readonly(amm_config, false),          // 1: amm_config
        AccountMeta::new(pool.address, false),                 // 2: pool_state
        AccountMeta::new(user_input_ata, false),               // 3: input_token_account
        AccountMeta::new(user_output_ata, false),              // 4: output_token_account
        AccountMeta::new(input_vault, false),                  // 5: input_vault
        AccountMeta::new(output_vault, false),                 // 6: output_vault
        AccountMeta::new(observation_state, false),            // 7: observation_state
        AccountMeta::new_readonly(token_program, false),       // 8: token_program
        AccountMeta::new_readonly(token_2022_program, false),  // 9: token_program_2022
        AccountMeta::new_readonly(memo_program, false),        // 10: memo_program
        AccountMeta::new_readonly(input_mint, false),          // 11: input_vault_mint
        AccountMeta::new_readonly(output_mint, false),         // 12: output_vault_mint
        // Remaining accounts:
        AccountMeta::new(bitmap_extension, false),             // 13: bitmap extension
        AccountMeta::new(tick_arrays[0], false),               // 14: tick_array_0
        AccountMeta::new(tick_arrays[1], false),               // 15: tick_array_1
        AccountMeta::new(tick_arrays[2], false),               // 16: tick_array_2
    ];

    Some(Instruction { program_id: clmm_program, accounts, data })
}
