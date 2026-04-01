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
}

impl DexType {
    /// Base fee in basis points for each DEX type.
    /// Used for rough profit estimation before precise simulation.
    pub fn base_fee_bps(&self) -> u64 {
        match self {
            DexType::RaydiumAmm => 25,      // 0.25%
            DexType::RaydiumClmm => 1,      // varies, 0.01% - 1% depending on pool
            DexType::RaydiumCp => 25,       // 0.25% constant product
            DexType::OrcaWhirlpool => 1,    // varies by fee tier
            DexType::MeteoraDlmm => 1,      // dynamic fees
            DexType::MeteoraDammV2 => 15,   // 0.15%
            DexType::SanctumInfinity => 3,  // ~3bps flat fee
        }
    }
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
}

impl PoolState {
    /// Compute output amount for a swap.
    /// Uses CLMM single-tick math when sqrt_price + liquidity are available,
    /// otherwise falls back to constant-product AMM math.
    pub fn get_output_amount(&self, input_amount: u64, a_to_b: bool) -> Option<u64> {
        if input_amount == 0 {
            return Some(0);
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

    /// Single-tick CLMM output calculation using f64.
    /// For Orca Whirlpool, Raydium CLMM, Meteora DLMM, DAMM v2 concentrated.
    ///
    /// Uses the standard concentrated liquidity formulas:
    ///   a_to_b: new_p = L*P / (L + input*P/Q), output = L*(P - new_p)/Q
    ///   b_to_a: new_p = P + input*Q/L, output = L*Q*(new_p - P)/(P*new_p)
    /// where P = sqrt_price_x64, L = liquidity, Q = 2^64
    fn get_clmm_output(
        &self,
        input_amount: u64,
        a_to_b: bool,
        sqrt_price_x64: u128,
        liquidity: u128,
    ) -> Option<u64> {
        let p = sqrt_price_x64 as f64;
        let l = liquidity as f64;
        let q: f64 = (1u128 << 64) as f64;

        let fee_factor = (10_000.0 - self.fee_bps as f64) / 10_000.0;
        let input_f = input_amount as f64 * fee_factor;

        if a_to_b {
            // Sell token A (base), get token B (quote). Price moves down.
            let denom = l + input_f * p / q;
            if denom <= 0.0 { return None; }
            let new_p = l * p / denom;
            if new_p <= 0.0 || new_p >= p { return None; }
            let output = l * (p - new_p) / q;
            if output <= 0.0 || output > u64::MAX as f64 { return None; }
            Some(output as u64)
        } else {
            // Sell token B (quote), get token A (base). Price moves up.
            let new_p = p + input_f * q / l;
            if new_p <= p { return None; }
            let output = l * q * (new_p - p) / (p * new_p);
            if output <= 0.0 || output > u64::MAX as f64 { return None; }
            Some(output as u64)
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
    /// The transaction signature
    pub signature: String,
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
