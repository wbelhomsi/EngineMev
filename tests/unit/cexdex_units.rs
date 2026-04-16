//! Unit tests for cexdex::units decimal conversion helpers.

use solana_mev_bot::cexdex::units::*;

#[test]
fn test_sol_lamports_roundtrip() {
    assert_eq!(sol_to_lamports(1.0), 1_000_000_000);
    assert_eq!(sol_to_lamports(0.5), 500_000_000);
    assert_eq!(lamports_to_sol(1_000_000_000), 1.0);
    assert_eq!(lamports_to_sol(500_000_000), 0.5);
}

#[test]
fn test_usdc_atoms_roundtrip() {
    assert_eq!(usdc_to_atoms(1.0), 1_000_000);
    assert_eq!(usdc_to_atoms(185.20), 185_200_000);
    assert_eq!(atoms_to_usdc(1_000_000), 1.0);
    assert_eq!(atoms_to_usdc(185_200_000), 185.20);
}

#[test]
fn test_sol_to_usdc_atoms_at_price() {
    assert_eq!(sol_to_usdc_atoms(1_000_000_000, 185.0), 185_000_000);
    assert_eq!(sol_to_usdc_atoms(500_000_000, 200.0), 100_000_000);
}

#[test]
fn test_usdc_atoms_to_sol_lamports_at_price() {
    assert_eq!(usdc_atoms_to_sol_lamports(185_000_000, 185.0), 1_000_000_000);
    assert_eq!(usdc_atoms_to_sol_lamports(100_000_000, 200.0), 500_000_000);
}

#[test]
fn test_zero_amounts() {
    assert_eq!(sol_to_lamports(0.0), 0);
    assert_eq!(usdc_to_atoms(0.0), 0);
    assert_eq!(lamports_to_sol(0), 0.0);
    assert_eq!(atoms_to_usdc(0), 0.0);
}

#[test]
fn test_large_amounts() {
    assert_eq!(sol_to_lamports(1000.0), 1_000_000_000_000);
    assert_eq!(usdc_to_atoms(1_000_000.0), 1_000_000_000_000);
}

#[test]
fn test_bps_to_fraction() {
    assert_eq!(bps_to_fraction(0), 0.0);
    assert_eq!(bps_to_fraction(100), 0.01);
    assert_eq!(bps_to_fraction(10_000), 1.0);
    assert_eq!(bps_to_fraction(15), 0.0015);
}

#[test]
fn test_spread_bps() {
    let bps = spread_bps(100.0, 100.5);
    assert_eq!(bps, 50);
    let bps2 = spread_bps(100.5, 100.0);
    assert_eq!(bps2, 50);
    assert_eq!(spread_bps(100.0, 100.0), 0);
    assert_eq!(spread_bps(0.0, 100.0), 0);
}
