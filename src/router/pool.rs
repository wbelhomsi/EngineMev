use solana_sdk::pubkey::Pubkey;

/// Supported DEX types on Solana.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DexType {
    RaydiumAmm,
    RaydiumClmm,
    OrcaWhirlpool,
    MeteoraDlmm,
}

impl DexType {
    /// Base fee in basis points for each DEX type.
    /// Used for rough profit estimation before precise simulation.
    pub fn base_fee_bps(&self) -> u64 {
        match self {
            DexType::RaydiumAmm => 25,     // 0.25%
            DexType::RaydiumClmm => 1,     // varies, 0.01% - 1% depending on pool
            DexType::OrcaWhirlpool => 1,   // varies by fee tier
            DexType::MeteoraDlmm => 1,     // dynamic fees
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
    /// Compute output amount for a constant-product AMM swap.
    /// For CLMM pools, this is a rough approximation — the simulator
    /// does the precise tick-crossing math.
    pub fn get_output_amount(&self, input_amount: u64, a_to_b: bool) -> Option<u64> {
        if input_amount == 0 {
            return Some(0);
        }

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
