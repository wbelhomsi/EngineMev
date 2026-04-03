use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use solana_mev_bot::config;
use solana_mev_bot::executor::bundle::build_sanctum_swap_ix;

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
fn test_sanctum_swap_ix_shank_discriminant() {
    let signer = Pubkey::new_unique();
    let jitosol = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let wsol = config::sol_mint();

    let ix = build_sanctum_swap_ix(
        &signer, &jitosol, &wsol,
        1_000_000_000, 990_000_000,
        12, 1, // jitoSOL=12, wSOL=1
    ).expect("Should build");

    // Shank discriminant = 1 byte, value 0x01
    assert_eq!(ix.data[0], 0x01, "Discriminant must be 0x01 (Shank, not Anchor)");
    assert_eq!(ix.data.len(), 27, "Data must be 27 bytes");
}

#[test]
fn test_sanctum_swap_ix_data_layout() {
    let signer = Pubkey::new_unique();
    let jitosol = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let wsol = config::sol_mint();

    let ix = build_sanctum_swap_ix(
        &signer, &jitosol, &wsol,
        2_000_000_000, 1_900_000_000,
        12, 1,
    ).unwrap();

    // src_lst_value_calc_accs at byte 1 (jitoSOL uses SPL calc = 5 accs)
    assert_eq!(ix.data[1], 5);
    // dst_lst_value_calc_accs at byte 2 (wSOL = 1 acc)
    assert_eq!(ix.data[2], 1);
    // src_lst_index at bytes 3..7
    assert_eq!(u32::from_le_bytes(ix.data[3..7].try_into().unwrap()), 12);
    // dst_lst_index at bytes 7..11
    assert_eq!(u32::from_le_bytes(ix.data[7..11].try_into().unwrap()), 1);
    // min_amount_out at bytes 11..19
    assert_eq!(u64::from_le_bytes(ix.data[11..19].try_into().unwrap()), 1_900_000_000);
    // amount at bytes 19..27
    assert_eq!(u64::from_le_bytes(ix.data[19..27].try_into().unwrap()), 2_000_000_000);
}

#[test]
fn test_sanctum_swap_ix_account_count_jitosol_to_wsol() {
    let signer = Pubkey::new_unique();
    let jitosol = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let wsol = config::sol_mint();

    let ix = build_sanctum_swap_ix(
        &signer, &jitosol, &wsol,
        1_000_000_000, 900_000_000,
        12, 1,
    ).unwrap();

    // 12 fixed + 5 src(SPL) + 1 dst(wSOL) + 2 pricing = 20
    assert_eq!(ix.accounts.len(), 20, "jitoSOL->wSOL needs 20 accounts");
    assert!(ix.accounts[0].is_signer);
}

#[test]
fn test_sanctum_swap_ix_account_count_wsol_to_msol() {
    let signer = Pubkey::new_unique();
    let wsol = config::sol_mint();
    let msol = Pubkey::from_str("mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So").unwrap();

    let ix = build_sanctum_swap_ix(
        &signer, &wsol, &msol,
        1_000_000_000, 900_000_000,
        1, 17, // wSOL=1, mSOL=17
    ).unwrap();

    // 12 fixed + 1 src(wSOL) + 5 dst(Marinade) + 2 pricing = 20
    assert_eq!(ix.accounts.len(), 20, "wSOL->mSOL needs 20 accounts");
}

#[test]
fn test_sanctum_swap_ix_program_id() {
    let signer = Pubkey::new_unique();
    let jitosol = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let wsol = config::sol_mint();

    let ix = build_sanctum_swap_ix(&signer, &jitosol, &wsol, 1000, 900, 12, 1).unwrap();
    assert_eq!(ix.program_id, config::programs::sanctum_s_controller());
}
