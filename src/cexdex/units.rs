//! Decimal conversion helpers for SOL (9 decimals), USDC (6 decimals),
//! and USD prices (f64). All callers MUST use these helpers — never do
//! raw decimal math elsewhere.

pub const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
pub const ATOMS_PER_USDC: u64 = 1_000_000;

#[inline]
pub fn sol_to_lamports(sol: f64) -> u64 {
    (sol * LAMPORTS_PER_SOL as f64) as u64
}

#[inline]
pub fn lamports_to_sol(lamports: u64) -> f64 {
    lamports as f64 / LAMPORTS_PER_SOL as f64
}

#[inline]
pub fn usdc_to_atoms(usdc: f64) -> u64 {
    (usdc * ATOMS_PER_USDC as f64) as u64
}

#[inline]
pub fn atoms_to_usdc(atoms: u64) -> f64 {
    atoms as f64 / ATOMS_PER_USDC as f64
}

/// Convert SOL lamports to USDC atoms at a given price (USD per SOL).
#[inline]
pub fn sol_to_usdc_atoms(sol_lamports: u64, price_usd_per_sol: f64) -> u64 {
    let sol = lamports_to_sol(sol_lamports);
    let usdc = sol * price_usd_per_sol;
    usdc_to_atoms(usdc)
}

/// Convert USDC atoms to SOL lamports at a given price (USD per SOL).
#[inline]
pub fn usdc_atoms_to_sol_lamports(usdc_atoms: u64, price_usd_per_sol: f64) -> u64 {
    if price_usd_per_sol <= 0.0 {
        return 0;
    }
    let usdc = atoms_to_usdc(usdc_atoms);
    let sol = usdc / price_usd_per_sol;
    sol_to_lamports(sol)
}

/// Convert basis points (1 bp = 0.01%) to a fraction.
#[inline]
pub fn bps_to_fraction(bps: u64) -> f64 {
    bps as f64 / 10_000.0
}

/// Compute the absolute spread between two prices in basis points.
/// Reference = first argument. Returns 0 if reference is 0.
#[inline]
pub fn spread_bps(reference: f64, other: f64) -> u64 {
    if reference <= 0.0 {
        return 0;
    }
    let diff = (other - reference).abs();
    ((diff / reference) * 10_000.0).round() as u64
}
