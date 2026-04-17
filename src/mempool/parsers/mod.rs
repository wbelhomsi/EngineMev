pub mod nonce;
pub mod orca;
pub mod raydium_clmm;
pub mod raydium_amm;
pub mod raydium_cp;
pub mod meteora_dlmm;
pub mod meteora_damm_v2;
pub mod phoenix;
pub mod manifest;
pub mod pumpswap;

pub use nonce::parse_nonce;
pub use orca::parse_orca_whirlpool;
pub use raydium_clmm::parse_raydium_clmm;
pub use raydium_amm::parse_raydium_amm_v4;
pub use raydium_cp::parse_raydium_cp;
pub use meteora_dlmm::parse_meteora_dlmm;
pub use meteora_damm_v2::parse_meteora_damm_v2;
pub use phoenix::{parse_phoenix_market, try_parse_orderbook};
pub use manifest::parse_manifest_market;
pub use pumpswap::parse_pumpswap;

/// Approximate token reserves from a CLMM sqrt_price_x64 + liquidity.
///
/// reserve_a ≈ L / (sqrt_price / 2^64)  = L * 2^64 / sqrt_price
/// reserve_b ≈ L * sqrt_price / 2^64
pub fn approx_reserves_from_sqrt_price(sqrt_price_x64: u128, liquidity: u128) -> (u64, u64) {
    if sqrt_price_x64 == 0 || liquidity == 0 {
        return (0, 0);
    }
    let q64: u128 = 1u128 << 64;
    let reserve_a = liquidity
        .checked_mul(q64)
        .and_then(|v| v.checked_div(sqrt_price_x64))
        .unwrap_or(0);
    let reserve_b = liquidity
        .checked_mul(sqrt_price_x64)
        .and_then(|v| v.checked_div(q64))
        .unwrap_or(0);
    let ra = if reserve_a > u64::MAX as u128 { u64::MAX } else { reserve_a as u64 };
    let rb = if reserve_b > u64::MAX as u128 { u64::MAX } else { reserve_b as u64 };
    (ra, rb)
}
