pub mod manifest;
pub mod manifest_mm;
pub mod meteora_damm_v2;
pub mod meteora_dlmm;
pub mod orca;
pub mod phoenix;
pub mod pumpswap;
pub mod raydium_amm;
pub mod raydium_clmm;
pub mod raydium_cp;
pub mod sanctum;

pub use manifest::build_manifest_swap_ix;
pub use meteora_damm_v2::build_damm_v2_swap_ix;
pub use meteora_dlmm::build_meteora_dlmm_swap_ix;
pub use orca::build_orca_whirlpool_swap_ix;
pub use phoenix::build_phoenix_swap_ix;
pub use pumpswap::build_pumpswap_swap_ix;
pub use raydium_amm::build_raydium_amm_swap_ix;
pub use raydium_clmm::build_raydium_clmm_swap_ix;
pub use raydium_cp::build_raydium_cp_swap_ix;
pub use sanctum::build_sanctum_swap_ix;

use solana_sdk::pubkey::Pubkey;
use crate::addresses;

// ─── Shared helpers used by multiple DEX builders ────────────────────────────

/// Derive an Associated Token Account address (SPL Token only).
/// ATA = PDA([wallet, TOKEN_PROGRAM_ID, mint], ATA_PROGRAM_ID)
pub(crate) fn derive_ata(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    derive_ata_with_program(wallet, mint, &addresses::SPL_TOKEN)
}

pub(crate) fn derive_ata_with_program(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    let seeds = &[
        wallet.as_ref(),
        token_program.as_ref(),
        mint.as_ref(),
    ];
    let (ata, _) = Pubkey::find_program_address(seeds, &addresses::ATA_PROGRAM);
    ata
}

/// Floor division that rounds toward negative infinity (needed for tick array computation).
pub(crate) fn floor_div(dividend: i32, divisor: i32) -> i32 {
    if dividend % divisor == 0 || dividend.signum() == divisor.signum() {
        dividend / divisor
    } else {
        dividend / divisor - 1
    }
}
