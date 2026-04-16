use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

use crate::addresses;

use super::derive_ata;

/// Shank discriminant for Sanctum S Controller SwapExactIn (NOT Anchor).
const SANCTUM_SWAP_EXACT_IN_DISCM: u8 = 0x01;

/// Build a Sanctum Infinity SwapExactIn instruction (Shank format).
///
/// Data: 27 bytes = discm(1) + src_calc_accs(1) + dst_calc_accs(1)
///       + src_lst_index(4) + dst_lst_index(4) + min_amount_out(8) + amount(8)
/// Accounts: 12 fixed + variable remaining (calculator groups + pricing)
pub fn build_sanctum_swap_ix(
    signer: &Pubkey,
    input_mint: &Pubkey,
    output_mint: &Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
    src_lst_index: u32,
    dst_lst_index: u32,
) -> Option<Instruction> {
    let (src_calc_program, src_calc_suffix, src_calc_accs) =
        crate::config::sanctum_calculator_accounts(input_mint);
    let (dst_calc_program, dst_calc_suffix, dst_calc_accs) =
        crate::config::sanctum_calculator_accounts(output_mint);

    // 12 fixed accounts
    let mut accounts = sanctum_swap_accounts_v2(signer, input_mint, output_mint);

    // Group A: Source calculator remaining accounts
    accounts.push(AccountMeta::new_readonly(src_calc_program, false));
    for acc in &src_calc_suffix {
        accounts.push(AccountMeta::new_readonly(*acc, false));
    }

    // Group B: Destination calculator remaining accounts
    accounts.push(AccountMeta::new_readonly(dst_calc_program, false));
    for acc in &dst_calc_suffix {
        accounts.push(AccountMeta::new_readonly(*acc, false));
    }

    // Group C: Pricing program + state
    accounts.push(AccountMeta::new_readonly(addresses::SANCTUM_PRICING, false));
    accounts.push(AccountMeta::new_readonly(crate::config::sanctum_pricing_state(), false));

    // 27-byte Shank instruction data
    let mut data = Vec::with_capacity(27);
    data.push(SANCTUM_SWAP_EXACT_IN_DISCM);
    data.push(src_calc_accs);
    data.push(dst_calc_accs);
    data.extend_from_slice(&src_lst_index.to_le_bytes());
    data.extend_from_slice(&dst_lst_index.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    data.extend_from_slice(&amount_in.to_le_bytes());

    Some(Instruction {
        program_id: addresses::SANCTUM_S_CONTROLLER,
        accounts,
        data,
    })
}

/// Build the 12 fixed accounts for Sanctum SwapExactIn (Shank format).
pub(crate) fn sanctum_swap_accounts_v2(
    signer: &Pubkey,
    input_mint: &Pubkey,
    output_mint: &Pubkey,
) -> Vec<AccountMeta> {
    let s_controller = addresses::SANCTUM_S_CONTROLLER;
    let token_program = addresses::SPL_TOKEN;

    // PDAs
    let (pool_state_pda, _) = Pubkey::find_program_address(&[b"state"], &s_controller);
    let (lst_state_list_pda, _) = Pubkey::find_program_address(&[b"lst-state-list"], &s_controller);
    let (protocol_fee_pda, _) = Pubkey::find_program_address(&[b"protocol-fee"], &s_controller);

    // ATAs
    let user_src_ata = derive_ata(signer, input_mint);
    let user_dst_ata = derive_ata(signer, output_mint);
    let protocol_fee_accumulator = derive_ata(&protocol_fee_pda, output_mint);
    let src_pool_reserves = derive_ata(&pool_state_pda, input_mint);
    let dst_pool_reserves = derive_ata(&pool_state_pda, output_mint);

    vec![
        AccountMeta::new_readonly(*signer, true),              // 0: signer
        AccountMeta::new_readonly(*input_mint, false),         // 1: src_lst_mint
        AccountMeta::new_readonly(*output_mint, false),        // 2: dst_lst_mint
        AccountMeta::new(user_src_ata, false),                 // 3: src_lst_acc (writable)
        AccountMeta::new(user_dst_ata, false),                 // 4: dst_lst_acc (writable)
        AccountMeta::new(protocol_fee_accumulator, false),     // 5: protocol_fee_accumulator (writable)
        AccountMeta::new_readonly(token_program, false),       // 6: src_lst_token_program
        AccountMeta::new_readonly(token_program, false),       // 7: dst_lst_token_program
        AccountMeta::new(pool_state_pda, false),               // 8: pool_state (writable)
        AccountMeta::new(lst_state_list_pda, false),           // 9: lst_state_list (writable)
        AccountMeta::new(src_pool_reserves, false),            // 10: src_pool_reserves (writable)
        AccountMeta::new(dst_pool_reserves, false),            // 11: dst_pool_reserves (writable)
    ]
}
