use solana_mev_bot::config;

#[test]
fn test_lst_mints_parse() {
    let mints = config::lst_mints();
    assert_eq!(mints.len(), 3);
    assert_eq!(mints[0].1, "jitoSOL");
    assert_eq!(mints[1].1, "mSOL");
    assert_eq!(mints[2].1, "bSOL");

    // Verify pubkeys are valid (didn't panic during creation)
    for (pubkey, name) in &mints {
        assert_ne!(pubkey.to_string(), "", "Invalid pubkey for {}", name);
    }
}

#[test]
fn test_sol_value_calculator_mapping() {
    let mints = config::lst_mints();
    for (mint, name) in &mints {
        let calc = config::sanctum_sol_value_calculator(mint);
        assert!(calc.is_some(), "No SOL Value Calculator for {}", name);
    }
}

#[test]
fn test_sol_value_calculator_unknown_mint() {
    let unknown = solana_sdk::pubkey::Pubkey::new_unique();
    assert!(config::sanctum_sol_value_calculator(&unknown).is_none());
}

#[test]
fn test_sanctum_program_ids() {
    let s_controller = config::programs::sanctum_s_controller();
    assert_eq!(
        s_controller.to_string(),
        "5ocnV1qiCgaQR8Jb8xWnVbApfaygJ8tNoZfgPwsgx9kx"
    );

    let pricing = config::programs::sanctum_pricing();
    assert_eq!(
        pricing.to_string(),
        "s1b6NRXj6ygNu1QMKXh2H9LUR2aPApAAm1UQ2DjdhNV"
    );
}
