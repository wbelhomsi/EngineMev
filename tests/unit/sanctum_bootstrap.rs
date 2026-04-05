use std::time::Duration;

use solana_sdk::pubkey::Pubkey;

use solana_mev_bot::config;
use solana_mev_bot::router::pool::DexType;
use solana_mev_bot::sanctum::{bootstrap_pools, update_virtual_pool};
use solana_mev_bot::state::StateCache;

/// Helper: derive the virtual pool PDA the same way sanctum.rs does.
fn virtual_pool_addr(lst_mint: &Pubkey) -> Pubkey {
    let (addr, _) = Pubkey::find_program_address(
        &[b"sanctum-virtual", lst_mint.as_ref()],
        &solana_system_interface::program::id(),
    );
    addr
}

// ---------------------------------------------------------------------------
// bootstrap_pools
// ---------------------------------------------------------------------------

#[test]
fn test_bootstrap_pools_creates_virtual_pools() {
    let cache = StateCache::new(Duration::from_secs(60));
    bootstrap_pools(&cache);

    // Must create exactly one virtual pool per supported LST
    let lst_mints = config::lst_mints();
    assert_eq!(lst_mints.len(), 3, "expected 3 supported LSTs");
    assert_eq!(cache.len(), 3, "cache should contain exactly 3 pools");

    let sol = config::sol_mint();

    for (mint, name) in &lst_mints {
        let addr = virtual_pool_addr(mint);
        let pool = cache
            .get_any(&addr)
            .unwrap_or_else(|| panic!("virtual pool for {} not found in cache", name));

        assert_eq!(pool.dex_type, DexType::SanctumInfinity);
        assert_eq!(pool.token_a_mint, sol, "{} token_a should be SOL", name);
        assert_eq!(pool.token_b_mint, *mint, "{} token_b should be the LST mint", name);
        assert_eq!(pool.fee_bps, 3, "{} fee_bps should be 3", name);
        assert!(pool.token_a_reserve > 0, "{} reserve_a must be > 0", name);
        assert!(pool.token_b_reserve > 0, "{} reserve_b must be > 0", name);
        // reserve_b < reserve_a because rate > 1.0 for all LSTs
        assert!(
            pool.token_b_reserve < pool.token_a_reserve,
            "{}: reserve_b should be less than reserve_a (rate > 1)",
            name
        );
    }
}

#[test]
fn test_bootstrap_pools_idempotent() {
    let cache = StateCache::new(Duration::from_secs(60));
    bootstrap_pools(&cache);
    assert_eq!(cache.len(), 3);

    // Call again -- should upsert, not duplicate
    bootstrap_pools(&cache);
    assert_eq!(cache.len(), 3, "calling bootstrap_pools twice should not duplicate pools");
}

#[test]
fn test_bootstrap_pools_reserves_reflect_hardcoded_rates() {
    let cache = StateCache::new(Duration::from_secs(60));
    bootstrap_pools(&cache);

    let base: u64 = 1_000_000_000_000_000;

    let expected_rates: Vec<(&str, f64)> = vec![
        ("jitoSOL", 1.271),
        ("mSOL", 1.371),
        ("bSOL", 1.286),
    ];

    for (name, rate) in &expected_rates {
        let mint = config::lst_mints()
            .into_iter()
            .find(|(_, n)| n == name)
            .unwrap_or_else(|| panic!("LST {} not found in lst_mints()", name))
            .0;

        let addr = virtual_pool_addr(&mint);
        let pool = cache.get_any(&addr).unwrap();

        assert_eq!(pool.token_a_reserve, base, "{} reserve_a should equal base", name);

        let expected_b = (base as f64 / rate) as u64;
        assert_eq!(
            pool.token_b_reserve, expected_b,
            "{}: reserve_b mismatch (rate={})",
            name, rate
        );
    }
}

#[test]
fn test_bootstrap_pools_populates_token_index() {
    let cache = StateCache::new(Duration::from_secs(60));
    bootstrap_pools(&cache);

    let sol = config::sol_mint();
    // SOL should appear in 3 pools (one per LST)
    let sol_pools = cache.pools_for_token(&sol);
    assert_eq!(sol_pools.len(), 3, "SOL should index into all 3 virtual pools");

    // Each LST mint should appear in exactly 1 pool
    for (mint, name) in config::lst_mints() {
        let pools = cache.pools_for_token(&mint);
        assert_eq!(pools.len(), 1, "{} should index into exactly 1 pool", name);
    }
}

#[test]
fn test_bootstrap_pools_populates_pair_index() {
    let cache = StateCache::new(Duration::from_secs(60));
    bootstrap_pools(&cache);

    let sol = config::sol_mint();
    for (mint, name) in config::lst_mints() {
        let pair_pools = cache.pools_for_pair(&sol, &mint);
        assert_eq!(
            pair_pools.len(),
            1,
            "SOL/{} pair should have exactly 1 pool",
            name
        );

        // Order should not matter
        let pair_pools_rev = cache.pools_for_pair(&mint, &sol);
        assert_eq!(pair_pools, pair_pools_rev, "pair lookup must be order-independent");
    }
}

// ---------------------------------------------------------------------------
// update_virtual_pool
// ---------------------------------------------------------------------------

#[test]
fn test_update_virtual_pool_changes_reserves() {
    let cache = StateCache::new(Duration::from_secs(60));
    bootstrap_pools(&cache);

    let jitosol = config::lst_mints()
        .into_iter()
        .find(|(_, n)| *n == "jitoSOL")
        .unwrap()
        .0;

    let addr = virtual_pool_addr(&jitosol);
    let before = cache.get_any(&addr).unwrap();

    // Update with a significantly different rate
    let new_rate = 1.500;
    update_virtual_pool(&cache, &jitosol, new_rate);

    let after = cache.get_any(&addr).unwrap();
    let base: u64 = 1_000_000_000_000_000;
    let expected_b = (base as f64 / new_rate) as u64;

    assert_eq!(after.token_a_reserve, base);
    assert_eq!(after.token_b_reserve, expected_b);
    assert_ne!(
        before.token_b_reserve, after.token_b_reserve,
        "reserves should change after rate update"
    );
}

#[test]
fn test_update_virtual_pool_preserves_pool_metadata() {
    let cache = StateCache::new(Duration::from_secs(60));
    bootstrap_pools(&cache);

    let msol = config::lst_mints()
        .into_iter()
        .find(|(_, n)| *n == "mSOL")
        .unwrap()
        .0;

    let addr = virtual_pool_addr(&msol);
    let before = cache.get_any(&addr).unwrap();

    update_virtual_pool(&cache, &msol, 1.450);

    let after = cache.get_any(&addr).unwrap();
    assert_eq!(after.dex_type, before.dex_type);
    assert_eq!(after.token_a_mint, before.token_a_mint);
    assert_eq!(after.token_b_mint, before.token_b_mint);
    assert_eq!(after.fee_bps, before.fee_bps);
    assert_eq!(after.address, before.address);
}

#[test]
fn test_update_virtual_pool_no_op_for_unknown_mint() {
    let cache = StateCache::new(Duration::from_secs(60));
    bootstrap_pools(&cache);

    let fake_mint = Pubkey::new_unique();
    // Should not panic, just silently do nothing (pool not in cache)
    update_virtual_pool(&cache, &fake_mint, 2.0);

    // Cache should still have exactly 3 pools
    assert_eq!(cache.len(), 3);
}

// ---------------------------------------------------------------------------
// sanctum_calculator_accounts (config.rs)
// ---------------------------------------------------------------------------

#[test]
fn test_sanctum_calculator_accounts_jitosol() {
    let jitosol = config::lst_mints()
        .into_iter()
        .find(|(_, n)| *n == "jitoSOL")
        .unwrap()
        .0;

    let (calc, accounts, count) = config::sanctum_calculator_accounts(&jitosol);
    assert_eq!(count, 5, "jitoSOL should use SPL Stake Pool calculator with 5 accounts");
    // calc_state + stake_pool + program + program_data = 4 extra accounts
    assert_eq!(accounts.len(), 4, "jitoSOL should have 4 additional accounts");
    // Calculator program should not be zero
    assert_ne!(calc, Pubkey::default());
}

#[test]
fn test_sanctum_calculator_accounts_msol() {
    let msol = config::lst_mints()
        .into_iter()
        .find(|(_, n)| *n == "mSOL")
        .unwrap()
        .0;

    let (calc, accounts, count) = config::sanctum_calculator_accounts(&msol);
    // Marinade uses its own calculator
    assert!(count > 0, "mSOL should have at least 1 account");
    assert_ne!(calc, Pubkey::default());
    // Accounts should be non-empty for mSOL
    assert!(!accounts.is_empty(), "mSOL should have additional accounts");
}

#[test]
fn test_sanctum_calculator_accounts_bsol() {
    let bsol = config::lst_mints()
        .into_iter()
        .find(|(_, n)| *n == "bSOL")
        .unwrap()
        .0;

    let (calc, accounts, count) = config::sanctum_calculator_accounts(&bsol);
    assert_eq!(count, 5, "bSOL should use SPL Stake Pool calculator with 5 accounts");
    assert_eq!(accounts.len(), 4, "bSOL should have 4 additional accounts");
    assert_ne!(calc, Pubkey::default());
}

#[test]
fn test_sanctum_calculator_accounts_wsol_fallback() {
    let sol = config::sol_mint();
    let (calc, accounts, count) = config::sanctum_calculator_accounts(&sol);
    assert_eq!(count, 1, "wSOL should use wSOL calculator with 1 account");
    assert!(accounts.is_empty(), "wSOL should have no additional accounts");
    assert_ne!(calc, Pubkey::default());
}

#[test]
fn test_sanctum_calculator_accounts_unknown_mint() {
    let fake = Pubkey::new_unique();
    let (calc, accounts, count) = config::sanctum_calculator_accounts(&fake);
    // Unknown mints fall back to wSOL calculator
    assert_eq!(count, 1, "unknown mint should fallback to wSOL calculator");
    assert!(accounts.is_empty(), "unknown mint should have no additional accounts");
    assert_ne!(calc, Pubkey::default());
}

// ---------------------------------------------------------------------------
// sanctum_sol_value_calculator (config.rs)
// ---------------------------------------------------------------------------

#[test]
fn test_sanctum_sol_value_calculator_known_mints() {
    for (mint, name) in config::lst_mints() {
        let calc = config::sanctum_sol_value_calculator(&mint);
        assert!(
            calc.is_some(),
            "{} should have a known SOL value calculator",
            name
        );
    }
}

#[test]
fn test_sanctum_sol_value_calculator_unknown_mint() {
    let fake = Pubkey::new_unique();
    assert!(
        config::sanctum_sol_value_calculator(&fake).is_none(),
        "unknown mint should return None"
    );
}

// ---------------------------------------------------------------------------
// lst_mints consistency
// ---------------------------------------------------------------------------

#[test]
fn test_lst_mints_are_distinct() {
    let mints = config::lst_mints();
    let sol = config::sol_mint();

    for (mint, name) in &mints {
        assert_ne!(*mint, sol, "{} mint should not equal SOL mint", name);
        assert_ne!(*mint, Pubkey::default(), "{} mint should not be default", name);
    }

    // All mints should be unique
    let mint_set: std::collections::HashSet<Pubkey> = mints.iter().map(|(m, _)| *m).collect();
    assert_eq!(mint_set.len(), mints.len(), "all LST mints should be unique");
}
