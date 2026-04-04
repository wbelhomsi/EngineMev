use solana_sdk::pubkey::Pubkey;

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
    /// Uses CLMM single-tick math when sqrt_price + liquidity are available,
    /// otherwise falls back to constant-product AMM math.
    pub fn get_output_amount(&self, input_amount: u64, a_to_b: bool) -> Option<u64> {
        if input_amount == 0 {
            return Some(0);
        }

        // Orderbook DEXes: use bid/ask price directly
        if let Some(output) = self.get_orderbook_output(input_amount, a_to_b) {
            return Some(output);
        }

        // Use CLMM single-tick math if sqrt_price and liquidity are available.
        // This is accurate for trades that don't cross a tick boundary.
        // It's conservative (underestimates output for large trades) which is
        // safer than overestimating — we'd rather miss an opportunity than
        // submit a losing bundle.
        if let (Some(sqrt_price_x64), Some(liquidity)) = (self.sqrt_price_x64, self.liquidity) {
            if sqrt_price_x64 > 0 && liquidity > 0 {
                return self.get_clmm_output(input_amount, a_to_b, sqrt_price_x64, liquidity);
            }
        }

        // Constant-product AMM math (Raydium AMM v4, CP, DAMM v2 compounding)
        self.get_cpmm_output(input_amount, a_to_b)
    }

    /// Single-tick CLMM output calculation using u128 integer math.
    /// For Orca Whirlpool, Raydium CLMM, DAMM v2 concentrated.
    ///
    /// Fee rate uses 1,000,000 denominator (not 10,000 basis points).
    /// CLMM feeRate = fee_bps * 100 (e.g., 0.3% fee = fee_bps=30, feeRate=3000).
    ///
    /// Formulas (P = sqrt_price in Q64.64, L = liquidity, Q = 2^64):
    ///   a_to_b: new_P = L*P / (L + input*P/Q),  output = L*(P - new_P)/Q
    ///   b_to_a: new_P = P + input*Q/L,  output = L*(1/P - 1/new_P)*Q
    ///
    /// Returns None on overflow or zero output — conservative for route discovery.
    fn get_clmm_output(
        &self,
        input_amount: u64,
        a_to_b: bool,
        sqrt_price_x64: u128,
        liquidity: u128,
    ) -> Option<u64> {
        let q: u128 = 1u128 << 64;

        // Fee: CLMM uses 1,000,000 denominator. fee_bps * 100 converts to CLMM rate.
        let fee_rate = self.fee_bps as u128 * 100;
        let fee_denom: u128 = 1_000_000;
        let input_after_fee = (input_amount as u128)
            .checked_mul(fee_denom.checked_sub(fee_rate)?)?
            .checked_div(fee_denom)?;

        if input_after_fee == 0 {
            return Some(0);
        }

        if a_to_b {
            // Sell token A, get token B. sqrt_price goes down.
            // new_P = L * P / (L + input * P / Q)
            // Rearranged to avoid overflow: new_P = (L * P) / (L + input * P / Q)
            let input_x_price = input_after_fee
                .checked_mul(sqrt_price_x64)?
                .checked_div(q)?;
            let denom = liquidity.checked_add(input_x_price)?;
            if denom == 0 { return None; }
            let new_sqrt_price = liquidity
                .checked_mul(sqrt_price_x64)?
                .checked_div(denom)?;

            if new_sqrt_price >= sqrt_price_x64 { return None; }

            // output = L * (P - new_P) / Q
            let price_diff = sqrt_price_x64.checked_sub(new_sqrt_price)?;
            let output = liquidity
                .checked_mul(price_diff)?
                .checked_div(q)?;

            if output > u64::MAX as u128 { return None; }
            Some(output as u64)
        } else {
            // Sell token B, get token A. sqrt_price goes up.
            // new_P = P + input * Q / L
            let price_delta = input_after_fee
                .checked_mul(q)?
                .checked_div(liquidity)?;
            let new_sqrt_price = sqrt_price_x64.checked_add(price_delta)?;

            if new_sqrt_price <= sqrt_price_x64 { return None; }

            // output = L * Q * (new_P - P) / (P * new_P)
            // To avoid overflow of L * Q (which exceeds u128 when L > 2^64),
            // rearrange: output = L * (Q / P - Q / new_P)
            //                   = L * Q * price_delta / (P * new_P)
            // Since price_delta = input * Q / L, substitute:
            //   output = input * Q^2 / (P * new_P)
            // But Q^2 = 2^128 which overflows u128.
            //
            // Safe approach: split into (L * price_delta / P) * (Q / new_P)
            // First: L * price_delta may overflow, but try it:
            let numerator = liquidity.checked_mul(price_delta)?;
            // Then: numerator * Q / (P * new_P)
            // = (numerator / P) * (Q / new_P)
            let step1 = numerator.checked_div(sqrt_price_x64)?;
            let output = step1.checked_mul(q)?.checked_div(new_sqrt_price)?;

            if output > u64::MAX as u128 { return None; }
            Some(output as u64)
        }
    }

    /// Orderbook output calculation using top-of-book price.
    /// a_to_b = selling base into bids: output_quote = input_base * best_bid
    /// b_to_a = buying base from asks:  output_base = input_quote / best_ask
    ///
    /// Depth semantics:
    ///   a_to_b: token_a_reserve is the available base depth; cap input_base by it.
    ///   b_to_a: token_b_reserve is the available base-output depth; cap output_base by it.
    ///
    /// Manifest prices are D18 fixed-point (scaled by 10^18). For Manifest:
    ///   a_to_b: output = input * price_d18 / 10^18
    ///   b_to_a: output = input * 10^18 / price_d18
    fn get_orderbook_output(&self, input_amount: u64, a_to_b: bool) -> Option<u64> {
        const D18: u128 = 1_000_000_000_000_000_000;

        // Apply fee
        let input_after_fee = (input_amount as u128)
            .checked_mul(10_000u128.checked_sub(self.fee_bps as u128)?)?
            .checked_div(10_000)?;

        if a_to_b {
            let price = self.best_bid_price?;
            if price == 0 {
                return None;
            }
            // Cap input (base atoms) by available bid depth (token_a_reserve)
            let effective_input = std::cmp::min(input_after_fee, self.token_a_reserve as u128);
            let raw = effective_input.checked_mul(price)?;
            let output = if self.dex_type == DexType::Manifest {
                raw.checked_div(D18)?
            } else {
                raw
            };
            if output > u64::MAX as u128 {
                return None;
            }
            Some(output as u64)
        } else {
            let price = self.best_ask_price?;
            if price == 0 {
                return None;
            }
            // Compute uncapped output (base atoms)
            let output = if self.dex_type == DexType::Manifest {
                input_after_fee.checked_mul(D18)?.checked_div(price)?
            } else {
                input_after_fee.checked_div(price)?
            };
            // Cap output (base atoms) by available ask depth (token_b_reserve)
            let capped_output = std::cmp::min(output, self.token_b_reserve as u128);
            if capped_output > u64::MAX as u128 {
                return None;
            }
            Some(capped_output as u64)
        }
    }

    /// Constant-product AMM output: output = (R_out * input) / (R_in + input)
    fn get_cpmm_output(&self, input_amount: u64, a_to_b: bool) -> Option<u64> {
        let (reserve_in, reserve_out) = if a_to_b {
            (self.token_a_reserve, self.token_b_reserve)
        } else {
            (self.token_b_reserve, self.token_a_reserve)
        };

        if reserve_in == 0 || reserve_out == 0 {
            return None;
        }

        // Apply fee: input_after_fee = input * (10000 - fee_bps) / 10000
        let input_after_fee = (input_amount as u128)
            .checked_mul(10_000u128.checked_sub(self.fee_bps as u128)?)?;

        // Constant product: output = (reserve_out * input_after_fee) / (reserve_in * 10000 + input_after_fee)
        let numerator = (reserve_out as u128).checked_mul(input_after_fee)?;
        let denominator = (reserve_in as u128)
            .checked_mul(10_000)?
            .checked_add(input_after_fee)?;

        let output = numerator.checked_div(denominator)?;

        // Sanity: output can't exceed reserves
        if output >= reserve_out as u128 {
            return None;
        }

        Some(output as u64)
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
