use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

use crate::addresses;
use crate::router::pool::PoolState;

use super::{derive_ata, derive_ata_with_program};

/// Build a PumpSwap AMM swap instruction.
///
/// Sell (token -> SOL): 21 accounts, discriminator [51, 230, 133, 164, 1, 127, 131, 173]
/// Buy (SOL -> token): 23 accounts, discriminator [102, 6, 61, 18, 1, 218, 235, 234]
///
/// Determines direction from input_mint:
///   input_mint == token_a_mint (base) => Sell (base -> quote/SOL)
///   input_mint == token_b_mint (quote/SOL) => Buy (SOL -> base/token)
pub fn build_pumpswap_swap_ix(
    signer: &Pubkey,
    pool: &PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let extra = &pool.extra;
    let base_vault = extra.vault_a?;
    let quote_vault = extra.vault_b?;
    let coin_creator = extra.coin_creator?;

    let pumpswap_program = addresses::PUMPSWAP;
    let system_program = solana_system_interface::program::id();

    // Resolve base token program (may be Token-2022 for some memecoins)
    let base_token_program = extra.token_program_a.unwrap_or(addresses::SPL_TOKEN);
    let quote_token_program = extra.token_program_b.unwrap_or(addresses::SPL_TOKEN);

    // User ATAs
    let user_base_ata = derive_ata_with_program(signer, &pool.token_a_mint, &base_token_program);
    let user_quote_ata = derive_ata(signer, &pool.token_b_mint); // wSOL always SPL Token

    // Coin creator vault PDA: ["creator_vault", coin_creator] on PumpSwap
    let (coin_creator_vault_authority, _) = Pubkey::find_program_address(
        &[b"creator_vault", coin_creator.as_ref()],
        &pumpswap_program,
    );
    // Coin creator vault ATA: standard ATA from (authority, quote_mint) with SPL Token
    let coin_creator_vault_ata = derive_ata(&coin_creator_vault_authority, &pool.token_b_mint);

    // Protocol fee recipient (use first hardcoded address)
    let protocol_fee_recipient = addresses::PUMPSWAP_FEE_RECIPIENT_1;
    let protocol_fee_recipient_ata = derive_ata(&protocol_fee_recipient, &pool.token_b_mint);

    let is_sell = input_mint == pool.token_a_mint;

    if is_sell {
        // Sell: base_amount_in -> min_quote_amount_out
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&[51, 230, 133, 164, 1, 127, 131, 173]); // sell discriminator
        data.extend_from_slice(&amount_in.to_le_bytes());               // base_amount_in
        data.extend_from_slice(&minimum_amount_out.to_le_bytes());      // min_quote_amount_out

        let accounts = vec![
            AccountMeta::new(pool.address, false),                          //  0: pool
            AccountMeta::new(*signer, true),                                //  1: user (signer)
            AccountMeta::new_readonly(addresses::PUMPSWAP_GLOBAL_CONFIG, false), //  2: global_config
            AccountMeta::new_readonly(pool.token_a_mint, false),            //  3: base_mint
            AccountMeta::new_readonly(pool.token_b_mint, false),            //  4: quote_mint (wSOL)
            AccountMeta::new(user_base_ata, false),                         //  5: user_base_token_account
            AccountMeta::new(user_quote_ata, false),                        //  6: user_quote_token_account
            AccountMeta::new(base_vault, false),                            //  7: pool_base_token_account
            AccountMeta::new(quote_vault, false),                           //  8: pool_quote_token_account
            AccountMeta::new_readonly(protocol_fee_recipient, false),       //  9: protocol_fee_recipient
            AccountMeta::new(protocol_fee_recipient_ata, false),            // 10: protocol_fee_recipient_token_account
            AccountMeta::new_readonly(base_token_program, false),           // 11: base_token_program
            AccountMeta::new_readonly(quote_token_program, false),          // 12: quote_token_program
            AccountMeta::new_readonly(system_program, false),               // 13: system_program
            AccountMeta::new_readonly(addresses::ATA_PROGRAM, false),       // 14: associated_token_program
            AccountMeta::new_readonly(addresses::PUMPSWAP_EVENT_AUTHORITY, false), // 15: event_authority
            AccountMeta::new_readonly(pumpswap_program, false),             // 16: pumpswap_program
            AccountMeta::new(coin_creator_vault_ata, false),                // 17: coin_creator_vault_ata
            AccountMeta::new_readonly(coin_creator_vault_authority, false),  // 18: coin_creator_vault_authority
            AccountMeta::new_readonly(addresses::PUMPSWAP_FEE_CONFIG, false), // 19: fee_config
            AccountMeta::new_readonly(addresses::PUMPSWAP_FEE_PROGRAM, false), // 20: fee_program
        ];

        Some(Instruction { program_id: pumpswap_program, accounts, data })
    } else {
        // Buy: base_amount_out + max_quote_amount_in + track_volume
        let mut data = Vec::with_capacity(25);
        data.extend_from_slice(&[102, 6, 61, 18, 1, 218, 235, 234]); // buy discriminator
        data.extend_from_slice(&minimum_amount_out.to_le_bytes());    // base_amount_out (what we want)
        data.extend_from_slice(&amount_in.to_le_bytes());             // max_quote_amount_in
        data.push(0u8);                                                // track_volume = None (OptionBool)

        // User volume accumulator PDA: ["user_volume_accumulator", signer] on PumpSwap
        let (user_volume_accumulator, _) = Pubkey::find_program_address(
            &[b"user_volume_accumulator", signer.as_ref()],
            &pumpswap_program,
        );

        let accounts = vec![
            AccountMeta::new(pool.address, false),                          //  0: pool
            AccountMeta::new(*signer, true),                                //  1: user (signer)
            AccountMeta::new_readonly(addresses::PUMPSWAP_GLOBAL_CONFIG, false), //  2: global_config
            AccountMeta::new_readonly(pool.token_a_mint, false),            //  3: base_mint
            AccountMeta::new_readonly(pool.token_b_mint, false),            //  4: quote_mint (wSOL)
            AccountMeta::new(user_base_ata, false),                         //  5: user_base_token_account
            AccountMeta::new(user_quote_ata, false),                        //  6: user_quote_token_account
            AccountMeta::new(base_vault, false),                            //  7: pool_base_token_account
            AccountMeta::new(quote_vault, false),                           //  8: pool_quote_token_account
            AccountMeta::new_readonly(protocol_fee_recipient, false),       //  9: protocol_fee_recipient
            AccountMeta::new(protocol_fee_recipient_ata, false),            // 10: protocol_fee_recipient_token_account
            AccountMeta::new_readonly(base_token_program, false),           // 11: base_token_program
            AccountMeta::new_readonly(quote_token_program, false),          // 12: quote_token_program
            AccountMeta::new_readonly(system_program, false),               // 13: system_program
            AccountMeta::new_readonly(addresses::ATA_PROGRAM, false),       // 14: associated_token_program
            AccountMeta::new_readonly(addresses::PUMPSWAP_EVENT_AUTHORITY, false), // 15: event_authority
            AccountMeta::new_readonly(pumpswap_program, false),             // 16: pumpswap_program
            AccountMeta::new(coin_creator_vault_ata, false),                // 17: coin_creator_vault_ata
            AccountMeta::new_readonly(coin_creator_vault_authority, false),  // 18: coin_creator_vault_authority
            AccountMeta::new(addresses::PUMPSWAP_GLOBAL_VOLUME_ACCUMULATOR, false), // 19: global_volume_accumulator
            AccountMeta::new(user_volume_accumulator, false),               // 20: user_volume_accumulator
            AccountMeta::new_readonly(addresses::PUMPSWAP_FEE_CONFIG, false), // 21: fee_config
            AccountMeta::new_readonly(addresses::PUMPSWAP_FEE_PROGRAM, false), // 22: fee_program
        ];

        Some(Instruction { program_id: pumpswap_program, accounts, data })
    }
}
