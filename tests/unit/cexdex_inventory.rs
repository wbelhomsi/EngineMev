use solana_mev_bot::cexdex::route::ArbDirection;
use solana_mev_bot::cexdex::Inventory;

fn mk_inventory() -> Inventory {
    Inventory::new_for_test()
}

#[test]
fn test_initial_empty_balance() {
    let inv = mk_inventory();
    assert_eq!(inv.sol_lamports_available(), 0);
    assert_eq!(inv.usdc_atoms_available(), 0);
}

#[test]
fn test_set_balances_and_ratio_50_50() {
    let inv = mk_inventory();
    inv.set_on_chain(5_000_000_000, 925_000_000); // 5 SOL + 925 USDC @ $185
    inv.set_sol_price_usd(185.0);
    // 5 SOL * 185 = $925; USDC = $925 → ratio 0.5
    let r = inv.ratio();
    assert!((r - 0.5).abs() < 0.001, "expected 0.5, got {}", r);
}

#[test]
fn test_ratio_100_sol() {
    let inv = mk_inventory();
    inv.set_on_chain(5_000_000_000, 0);
    inv.set_sol_price_usd(185.0);
    assert_eq!(inv.ratio(), 1.0);
}

#[test]
fn test_ratio_100_usdc() {
    let inv = mk_inventory();
    inv.set_on_chain(0, 925_000_000);
    inv.set_sol_price_usd(185.0);
    assert_eq!(inv.ratio(), 0.0);
}

#[test]
fn test_allow_normal_zone_both_directions() {
    let inv = mk_inventory();
    inv.set_on_chain(5_000_000_000, 925_000_000);
    inv.set_sol_price_usd(185.0);
    assert!(inv.allows_direction(ArbDirection::BuyOnDex));
    assert!(inv.allows_direction(ArbDirection::SellOnDex));
    assert_eq!(inv.profit_multiplier(ArbDirection::BuyOnDex), 1.0);
    assert_eq!(inv.profit_multiplier(ArbDirection::SellOnDex), 1.0);
}

#[test]
fn test_skewed_sol_heavy_prefers_sell() {
    let inv = mk_inventory();
    // 70/30 SOL-heavy: 7 SOL @ $185 = $1295 SOL, 555 USDC → ratio = 0.7
    inv.set_on_chain(7_000_000_000, 555_000_000);
    inv.set_sol_price_usd(185.0);
    assert!(inv.allows_direction(ArbDirection::BuyOnDex));
    assert!(inv.allows_direction(ArbDirection::SellOnDex));
    assert_eq!(inv.profit_multiplier(ArbDirection::BuyOnDex), 2.0);
    assert_eq!(inv.profit_multiplier(ArbDirection::SellOnDex), 1.0);
}

#[test]
fn test_hard_cap_rejects_buy_when_sol_heavy() {
    let inv = mk_inventory();
    inv.set_on_chain(9_000_000_000, 185_000_000);
    inv.set_sol_price_usd(185.0);
    assert!(!inv.allows_direction(ArbDirection::BuyOnDex), "should block buy at 90% SOL");
    assert!(inv.allows_direction(ArbDirection::SellOnDex), "can still sell to rebalance");
}

#[test]
fn test_hard_cap_rejects_sell_when_usdc_heavy() {
    let inv = mk_inventory();
    inv.set_on_chain(1_000_000_000, 1_665_000_000);
    inv.set_sol_price_usd(185.0);
    assert!(inv.allows_direction(ArbDirection::BuyOnDex));
    assert!(!inv.allows_direction(ArbDirection::SellOnDex));
}

#[test]
fn test_reservation_lifecycle_commit() {
    let inv = mk_inventory();
    inv.set_on_chain(5_000_000_000, 925_000_000);

    inv.reserve(ArbDirection::SellOnDex, 1_000_000_000, 0);
    assert_eq!(inv.sol_lamports_available(), 4_000_000_000);

    inv.commit(ArbDirection::SellOnDex, 1_000_000_000, 185_000_000);
    assert_eq!(inv.sol_lamports_available(), 4_000_000_000);
    assert_eq!(inv.usdc_atoms_available(), 925_000_000 + 185_000_000);
}

#[test]
fn test_reservation_lifecycle_release() {
    let inv = mk_inventory();
    inv.set_on_chain(5_000_000_000, 925_000_000);

    inv.reserve(ArbDirection::SellOnDex, 1_000_000_000, 0);
    assert_eq!(inv.sol_lamports_available(), 4_000_000_000);

    inv.release(ArbDirection::SellOnDex, 1_000_000_000, 0);
    assert_eq!(inv.sol_lamports_available(), 5_000_000_000);
    assert_eq!(inv.usdc_atoms_available(), 925_000_000);
}
