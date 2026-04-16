use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

use crate::addresses;
use crate::router::pool::PoolState;

use super::derive_ata_with_program;

/// Build a Meteora DLMM swap2 instruction with full account layout.
///
/// Program: LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo
/// Discriminator: [0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88] (swap2)
/// Accounts: 15 fixed + N bin arrays as remaining accounts
pub fn build_meteora_dlmm_swap_ix(
    signer: &Pubkey,
    pool: &PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
    // Token programs from authoritative RPC source (not pool.extra flags)
    mint_a_program: Option<Pubkey>,
    mint_b_program: Option<Pubkey>,
) -> Option<Instruction> {
    let extra = &pool.extra;
    let vault_a = extra.vault_a?;  // reserve_x
    let vault_b = extra.vault_b?;  // reserve_y

    let dlmm_program = addresses::METEORA_DLMM;
    let token_program = addresses::SPL_TOKEN;
    let memo_program = addresses::MEMO;

    let a_to_b = input_mint == pool.token_a_mint; // X -> Y
    let active_id = pool.current_tick.unwrap_or(0);

    // Use authoritative token programs (from RPC cache), falling back to pool.extra
    let prog_a = mint_a_program
        .or(extra.token_program_a)
        .unwrap_or(token_program);
    let prog_b = mint_b_program
        .or(extra.token_program_b)
        .unwrap_or(token_program);
    let (input_prog, output_prog) = if a_to_b { (prog_a, prog_b) } else { (prog_b, prog_a) };
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };

    let user_input_ata = derive_ata_with_program(signer, &input_mint, &input_prog);
    let user_output_ata = derive_ata_with_program(signer, &output_mint, &output_prog);

    // Oracle PDA
    let (oracle, _) = Pubkey::find_program_address(
        &[b"oracle", pool.address.as_ref()], &dlmm_program,
    );

    // Event authority PDA
    let (event_authority, _) = Pubkey::find_program_address(
        &[b"__event_authority"], &dlmm_program,
    );

    // Bitmap extension: only needed when active_id is near the edge of internal bitmap range
    // (±512 bin array indices). Most pools don't need it. If the PDA doesn't exist on-chain,
    // just don't include it — the swap can't traverse beyond the internal bitmap but that's fine
    // for single-bin arbs. To properly support it, we'd need to check on-chain existence at
    // pool discovery time and store in PoolExtra.

    // Bin array PDAs: compute the current bin array index and get a few in the swap direction
    let bin_array_index = if active_id >= 0 || active_id % 70 == 0 {
        active_id / 70
    } else {
        active_id / 70 - 1
    };

    // Get 3 bin arrays in swap direction
    let bin_offsets: [i32; 3] = if a_to_b {
        [0, -1, -2] // X->Y, price goes down, bins decrease
    } else {
        [0, 1, 2]   // Y->X, price goes up, bins increase
    };

    let bin_arrays: Vec<Pubkey> = bin_offsets.iter().map(|&o| {
        let idx = (bin_array_index + o) as i64;
        Pubkey::find_program_address(
            &[b"bin_array", pool.address.as_ref(), &idx.to_le_bytes()],
            &dlmm_program,
        ).0
    }).collect();

    // Discriminator: swap2
    let mut data = Vec::with_capacity(28);
    data.extend_from_slice(&[0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88]);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    // remaining_accounts_info: empty Vec (Borsh: 4 bytes of 0)
    data.extend_from_slice(&0u32.to_le_bytes());

    // Bitmap extension — use cached value if available (confirmed on-chain).
    // If not cached, pass the DLMM program ID as Anchor's "None" marker.
    // Pools needing the bitmap but not having it will fail, but that's expected
    // (they can't be swapped without it). The bitmap_checked cache in stream.rs
    // tracks which pools have been checked.
    let bitmap_extension = extra.bitmap_extension.unwrap_or(dlmm_program);

    let mut accounts = vec![
        AccountMeta::new(pool.address, false),              // 0: lb_pair
        AccountMeta::new(bitmap_extension, false),          // 1: bin_array_bitmap_extension
        AccountMeta::new(vault_a, false),                   // 2: reserve_x
        AccountMeta::new(vault_b, false),                   // 3: reserve_y
        AccountMeta::new(user_input_ata, false),            // 4: user_token_in
        AccountMeta::new(user_output_ata, false),           // 5: user_token_out
        AccountMeta::new_readonly(pool.token_a_mint, false),// 6: token_x_mint
        AccountMeta::new_readonly(pool.token_b_mint, false),// 7: token_y_mint
        AccountMeta::new(oracle, false),                    // 8: oracle
        AccountMeta::new(dlmm_program, false),               // 9: host_fee_in (None — pass program ID for Option, must be writable per IDL)
        AccountMeta::new(*signer, true),                    // 10: user (signer)
        AccountMeta::new_readonly(prog_a, false),             // 11: token_x_program
        AccountMeta::new_readonly(prog_b, false),            // 12: token_y_program
        AccountMeta::new_readonly(memo_program, false),       // 13: memo_program
        AccountMeta::new_readonly(event_authority, false),   // 14: event_authority
        AccountMeta::new_readonly(dlmm_program, false),      // 15: program
    ];

    // Append bin arrays as remaining accounts
    for ba in &bin_arrays {
        accounts.push(AccountMeta::new(*ba, false));
    }

    Some(Instruction { program_id: dlmm_program, accounts, data })
}
