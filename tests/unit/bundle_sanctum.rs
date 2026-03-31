use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use solana_mev_bot::config;
use solana_mev_bot::executor::bundle::sanctum_swap_accounts;

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
