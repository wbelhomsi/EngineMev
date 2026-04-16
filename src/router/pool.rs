use solana_sdk::pubkey::Pubkey;

/// A single tick entry in a CLMM tick array.
#[derive(Debug, Clone)]
pub struct ClmmTick {
    pub tick_index: i32,
    /// Signed net liquidity change when crossing this tick left-to-right.
    /// Subtract when crossing right-to-left (a_to_b / price decreasing).
    pub liquidity_net: i128,
    /// Total liquidity referencing this tick. > 0 means initialized.
    pub liquidity_gross: u128,
}

/// A parsed CLMM tick array (Orca: 88 ticks, Raydium: 60 ticks).
#[derive(Debug, Clone)]
pub struct ClmmTickArray {
    pub start_tick_index: i32,
    pub ticks: Vec<ClmmTick>,
}

/// A single bin in a Meteora DLMM bin array.
/// On-chain layout: amountX (i64), amountY (i64), price (u128 Q64.64).
#[derive(Debug, Clone)]
pub struct DlmmBin {
    pub amount_x: u64,
    pub amount_y: u64,
    /// Pre-stored Q64.64 fixed-point price.
    /// X->Y: out = in * price >> 64.
    /// Y->X: out = in << 64 / price.
    pub price_q64: u128,
}

/// Maximum number of bins per bin array account.
pub const DLMM_MAX_BIN_PER_ARRAY: usize = 70;

/// A parsed DLMM bin array (70 bins per array).
#[derive(Debug, Clone)]
pub struct DlmmBinArray {
    pub index: i64,
    pub bins: Vec<DlmmBin>,
}

/// Supported DEX types on Solana.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DexType {
    RaydiumAmm,
    RaydiumClmm,
    RaydiumCp,
    OrcaWhirlpool,
    MeteoraDlmm,
    MeteoraDammV2,
    SanctumInfinity,
    Phoenix,
    Manifest,
    PumpSwap,
}

impl DexType {
    /// Base fee in basis points for each DEX type.
    /// Used for rough profit estimation before precise simulation.
    pub fn base_fee_bps(&self) -> u64 {
        match self {
            DexType::RaydiumAmm => 25,      // 0.25%
            DexType::RaydiumClmm => 25,     // varies 1-100 bps, default 25 (most common tier)
            DexType::RaydiumCp => 25,       // 0.25% constant product
            DexType::OrcaWhirlpool => 1,    // varies by fee tier
            DexType::MeteoraDlmm => 1,      // dynamic fees
            DexType::MeteoraDammV2 => 15,   // 0.15%
            DexType::SanctumInfinity => 3,  // ~3bps flat fee
            DexType::Phoenix => 2,          // ~2bps taker fee on major markets
            DexType::Manifest => 0,         // zero fees
            DexType::PumpSwap => 125,       // conservative worst-case (tiered 30-125 bps)
        }
    }
}

/// Extra pool data needed for building swap instructions.
#[derive(Debug, Clone, Default)]
pub struct PoolExtra {
    pub vault_a: Option<Pubkey>,
    pub vault_b: Option<Pubkey>,
    pub config: Option<Pubkey>,
    pub token_program_a: Option<Pubkey>,
    pub token_program_b: Option<Pubkey>,
    /// Tick spacing for CLMM pools (Orca Whirlpool, Raydium CLMM)
    pub tick_spacing: Option<u16>,
    /// Observation state account (Raydium CLMM)
    pub observation: Option<Pubkey>,
    /// Bitmap extension account (Meteora DLMM) — None if doesn't exist on-chain
    pub bitmap_extension: Option<Pubkey>,
    /// Open orders account (Raydium AMM v4) — from pool state offset 496
    pub open_orders: Option<Pubkey>,
    /// Market account (Raydium AMM v4) — from pool state offset 528
    pub market: Option<Pubkey>,
    /// Market program (Raydium AMM v4) — from pool state offset 560 (OpenBook/Serum)
    pub market_program: Option<Pubkey>,
    /// Target orders account (Raydium AMM v4) — from pool state offset 592
    pub target_orders: Option<Pubkey>,
    /// AMM authority nonce (Raydium AMM v4) — from pool state offset 8
    pub amm_nonce: Option<u8>,
    /// Coin creator address (PumpSwap) — used for creator vault PDA
    pub coin_creator: Option<Pubkey>,
    /// Whether mayhem mode is active (PumpSwap)
    pub is_mayhem_mode: Option<bool>,
    /// Whether the coin participates in cashback program (PumpSwap)
    pub is_cashback_coin: Option<bool>,
}

/// Represents the on-chain state of an AMM pool.
/// This is the core data structure — route calculation and profit
/// simulation both read from this.
#[derive(Debug, Clone)]
pub struct PoolState {
    pub address: Pubkey,
    pub dex_type: DexType,
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub token_a_reserve: u64,
    pub token_b_reserve: u64,
    /// Fee in basis points (actual, not the DEX default)
    pub fee_bps: u64,
    /// For CLMM pools: current tick index
    pub current_tick: Option<i32>,
    /// For CLMM pools: sqrt price x64
    pub sqrt_price_x64: Option<u128>,
    /// For CLMM pools: available liquidity at current tick
    pub liquidity: Option<u128>,
    /// Slot when this state was last observed
    pub last_slot: u64,
    /// Extra data for building swap instructions (vaults, config, token programs)
    pub extra: PoolExtra,
    /// For orderbook DEXes: best bid in quote atoms per base atom
    pub best_bid_price: Option<u128>,
    /// For orderbook DEXes: best ask in quote atoms per base atom
    pub best_ask_price: Option<u128>,
}

impl PoolState {
    /// Compute output amount for a swap.
    /// Dispatches to per-DEX quoting math in `router::dex::*`.
    pub fn get_output_amount(&self, input_amount: u64, a_to_b: bool) -> Option<u64> {
        self.get_output_amount_with_cache(input_amount, a_to_b, None, None)
    }

    /// Compute output amount for a swap, using DLMM bin arrays when available.
    /// Falls back to `get_output_amount` when bin data is not provided or not applicable.
    pub fn get_output_amount_with_bins(
        &self,
        input_amount: u64,
        a_to_b: bool,
        bin_arrays: Option<&[DlmmBinArray]>,
    ) -> Option<u64> {
        self.get_output_amount_with_cache(input_amount, a_to_b, bin_arrays, None)
    }

    /// Compute output amount using all available cache data.
    /// Uses CLMM tick arrays for multi-tick crossing, DLMM bin arrays for bin-by-bin,
    /// and falls back to single-tick / constant-product math when cache data is absent.
    pub fn get_output_amount_with_cache(
        &self,
        input_amount: u64,
        a_to_b: bool,
        bin_arrays: Option<&[DlmmBinArray]>,
        tick_arrays: Option<&[ClmmTickArray]>,
    ) -> Option<u64> {
        if input_amount == 0 {
            return Some(0);
        }

        use crate::router::dex;
        match self.dex_type {
            DexType::MeteoraDlmm => dex::dlmm::quote(self, input_amount, a_to_b, bin_arrays)
                .or_else(|| dex::cpmm::quote(self, input_amount, a_to_b)),
            DexType::OrcaWhirlpool => dex::clmm_orca::quote(self, input_amount, a_to_b, tick_arrays)
                .or_else(|| dex::cpmm::quote(self, input_amount, a_to_b)),
            DexType::RaydiumClmm => dex::clmm_raydium::quote(self, input_amount, a_to_b, tick_arrays)
                .or_else(|| dex::cpmm::quote(self, input_amount, a_to_b)),
            DexType::Phoenix => dex::phoenix::quote(self, input_amount, a_to_b),
            DexType::Manifest => dex::manifest::quote(self, input_amount, a_to_b),
            DexType::MeteoraDammV2 => dex::damm_v2::quote(self, input_amount, a_to_b),
            DexType::SanctumInfinity => dex::sanctum::quote(self, input_amount, a_to_b),
            _ => dex::cpmm::quote(self, input_amount, a_to_b),
        }
    }

    /// DLMM bin-by-bin swap simulation (public for tests).
    /// Delegates to `dex::dlmm` module with explicit active_id.
    pub fn get_dlmm_bin_output(
        &self,
        input_amount: u64,
        a_to_b: bool,
        active_id: i32,
        bin_arrays: &[DlmmBinArray],
    ) -> Option<u64> {
        if input_amount == 0 {
            return Some(0);
        }
        crate::router::dex::dlmm::quote_with_active_id(
            self, input_amount, a_to_b, active_id, bin_arrays,
        )
    }

    /// Check if this pool contains the given token mint on either side.
    pub fn has_token(&self, mint: &Pubkey) -> bool {
        self.token_a_mint == *mint || self.token_b_mint == *mint
    }

    /// Given one token in the pair, return the other.
    pub fn other_token(&self, mint: &Pubkey) -> Option<Pubkey> {
        if self.token_a_mint == *mint {
            Some(self.token_b_mint)
        } else if self.token_b_mint == *mint {
            Some(self.token_a_mint)
        } else {
            None
        }
    }

    /// Determine swap direction for a given input mint.
    pub fn is_a_to_b(&self, input_mint: &Pubkey) -> Option<bool> {
        if self.token_a_mint == *input_mint {
            Some(true)
        } else if self.token_b_mint == *input_mint {
            Some(false)
        } else {
            None
        }
    }
}

/// A single hop in an arbitrage route.
#[derive(Debug, Clone)]
pub struct RouteHop {
    pub pool_address: Pubkey,
    pub dex_type: DexType,
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    /// Estimated output from this hop (filled during simulation)
    pub estimated_output: u64,
}

/// A complete circular arbitrage route.
/// Must start and end with the same token (e.g., SOL -> X -> SOL).
#[derive(Debug, Clone)]
pub struct ArbRoute {
    pub hops: Vec<RouteHop>,
    /// The token we start and end with
    pub base_mint: Pubkey,
    /// Amount of base token to input
    pub input_amount: u64,
    /// Estimated profit in base token (output - input)
    pub estimated_profit: i64,
    /// Estimated profit in lamports (for tip calculation)
    pub estimated_profit_lamports: u64,
}

impl ArbRoute {
    pub fn is_profitable(&self) -> bool {
        self.estimated_profit > 0
    }

    pub fn hop_count(&self) -> usize {
        self.hops.len()
    }

    /// Validate the route forms a valid circle.
    pub fn is_circular(&self) -> bool {
        if self.hops.is_empty() {
            return false;
        }
        let first_input = self.hops.first().unwrap().input_mint;
        let last_output = self.hops.last().unwrap().output_mint;
        first_input == last_output && first_input == self.base_mint
    }
}

/// Represents a detected swap in the mempool that we might backrun.
#[derive(Debug, Clone)]
pub struct DetectedSwap {
    /// Which DEX the swap targets
    pub dex_type: DexType,
    /// Pool being swapped on
    pub pool_address: Pubkey,
    /// Input token mint
    pub input_mint: Pubkey,
    /// Output token mint
    pub output_mint: Pubkey,
    /// Estimated swap amount (if decodable)
    pub amount: Option<u64>,
    /// The slot this was observed in
    pub observed_slot: u64,
}

// Re-export tick_index_to_sqrt_price_x64 for backward compatibility
// (tests and other modules import it from pool.rs).
pub use crate::router::dex::tick_index_to_sqrt_price_x64;
