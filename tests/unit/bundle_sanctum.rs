use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use solana_mev_bot::config;
use solana_mev_bot::executor::bundle::{build_sanctum_swap_ix, sanctum_swap_accounts};

#[test]
fn test_sanctum_pda_derivation() {
    let s_controller = config::programs::sanctum_s_controller();

    // Pool State PDA: seeds = [b"state"]
    let (pool_state_pda, _) = Pubkey::find_program_address(&[b"state"], &s_controller);

    // LST State List PDA: seeds = [b"lst-state-list"]
    let (lst_state_list_pda, _) = Pubkey::find_program_address(&[b"lst-state-list"], &s_controller);

    // These should be deterministic
    assert_ne!(pool_state_pda, lst_state_list_pda);
    assert_ne!(pool_state_pda, Pubkey::default());
}

#[test]
fn test_sanctum_swap_accounts_count() {
    let signer = Pubkey::new_unique();
    let jitosol_mint = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let sol_mint = config::sol_mint();

    let accounts = sanctum_swap_accounts(
        &signer,
        &jitosol_mint, // input
        &sol_mint,      // output
    );

    // SwapExactIn needs 12 accounts per spec
    assert_eq!(accounts.len(), 12, "Sanctum SwapExactIn requires 12 accounts");
}

#[test]
fn test_sanctum_swap_accounts_signer() {
    let signer = Pubkey::new_unique();
    let jitosol_mint = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let sol_mint = config::sol_mint();

    let accounts = sanctum_swap_accounts(&signer, &jitosol_mint, &sol_mint);

    // First account must be the signer
    assert!(accounts[0].is_signer, "First account must be signer");
    assert_eq!(accounts[0].pubkey, signer);
}

#[test]
fn test_sanctum_swap_ix_discriminator() {
    let signer = Pubkey::new_unique();
    let jitosol_mint = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let sol_mint = config::sol_mint();

    let ix = build_sanctum_swap_ix(
        &signer, &jitosol_mint, &sol_mint,
        1_000_000_000, 990_000_000,
    ).expect("Should build Sanctum swap IX");

    // Anchor discriminator for "swap_exact_in": sha256("global:swap_exact_in")[..8]
    let expected_disc: [u8; 8] = [0x68, 0x68, 0x83, 0x56, 0xa1, 0xbd, 0xb4, 0xd8];
    assert_eq!(&ix.data[..8], &expected_disc);
}

#[test]
fn test_sanctum_swap_ix_data_layout() {
    let signer = Pubkey::new_unique();
    let jitosol_mint = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let sol_mint = config::sol_mint();

    let amount_in: u64 = 2_500_000_000;
    let min_out: u64 = 2_300_000_000;

    let ix = build_sanctum_swap_ix(
        &signer, &jitosol_mint, &sol_mint,
        amount_in, min_out,
    ).expect("Should build Sanctum swap IX");

    assert_eq!(ix.data.len(), 24, "Instruction data must be exactly 24 bytes");
    let encoded_amount_in = u64::from_le_bytes(ix.data[8..16].try_into().unwrap());
    assert_eq!(encoded_amount_in, amount_in);
    let encoded_min_out = u64::from_le_bytes(ix.data[16..24].try_into().unwrap());
    assert_eq!(encoded_min_out, min_out);
}

#[test]
fn test_sanctum_swap_ix_program_id() {
    let signer = Pubkey::new_unique();
    let jitosol_mint = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let sol_mint = config::sol_mint();

    let ix = build_sanctum_swap_ix(
        &signer, &jitosol_mint, &sol_mint,
        1_000_000_000, 900_000_000,
    ).expect("Should build Sanctum swap IX");

    assert_eq!(ix.program_id, config::programs::sanctum_s_controller());
}

#[test]
fn test_sanctum_swap_ix_account_count_standalone() {
    let signer = Pubkey::new_unique();
    let jitosol_mint = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let sol_mint = config::sol_mint();

    let ix = build_sanctum_swap_ix(
        &signer, &jitosol_mint, &sol_mint,
        1_000_000_000, 900_000_000,
    ).expect("Should build Sanctum swap IX");

    assert_eq!(ix.accounts.len(), 12);
    assert!(ix.accounts[0].is_signer);
    assert_eq!(ix.accounts[0].pubkey, signer);
}
