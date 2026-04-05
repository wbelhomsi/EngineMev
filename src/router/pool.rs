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

        // DLMM with bin data
        if self.dex_type == DexType::MeteoraDlmm {
            if let (Some(active_id), Some(bins)) = (self.current_tick, bin_arrays) {
                if !bins.is_empty() {
                    return self
                        .get_dlmm_bin_output(input_amount, a_to_b, active_id, bins)
                        .or_else(|| self.get_output_amount(input_amount, a_to_b));
                }
            }
        }

        // CLMM with tick data — multi-tick crossing
        if matches!(self.dex_type, DexType::OrcaWhirlpool | DexType::RaydiumClmm) {
            if let (Some(sqrt_price), Some(liquidity), Some(ticks)) =
                (self.sqrt_price_x64, self.liquidity, tick_arrays)
            {
                if !ticks.is_empty() && sqrt_price > 0 && liquidity > 0 {
                    return self
                        .get_clmm_multi_tick_output(input_amount, a_to_b, sqrt_price, liquidity, ticks)
                        .or_else(|| self.get_output_amount(input_amount, a_to_b));
                }
            }
        }

        self.get_output_amount(input_amount, a_to_b)
    }

    /// Multi-tick CLMM swap simulation.
    /// Walks initialized ticks, adjusting liquidity at each boundary.
    /// More accurate than single-tick math for swaps that cross tick boundaries.
    ///
    /// Returns None on overflow or zero output — conservative for route discovery.
    fn get_clmm_multi_tick_output(
        &self,
        input_amount: u64,
        a_to_b: bool,
        sqrt_price_x64: u128,
        liquidity: u128,
        tick_arrays: &[ClmmTickArray],
    ) -> Option<u64> {
        let fee_rate = self.fee_bps as u128 * 100;
        let fee_denom: u128 = 1_000_000;

        // Apply fee upfront (matches single-tick approach)
        let mut amount_remaining = (input_amount as u128)
            .checked_mul(fee_denom.checked_sub(fee_rate)?)?
            .checked_div(fee_denom)?;

        if amount_remaining == 0 {
            return Some(0);
        }

        let mut total_output: u128 = 0;
        let mut current_sqrt_price = sqrt_price_x64;
        let mut current_liquidity = liquidity;
        let current_tick = self.current_tick.unwrap_or(0);

        // Collect all initialized ticks from the arrays, sorted for traversal
        let mut initialized_ticks: Vec<&ClmmTick> = tick_arrays
            .iter()
            .flat_map(|arr| arr.ticks.iter())
            .filter(|t| t.liquidity_gross > 0)
            .collect();

        if a_to_b {
            // Price decreasing: walk ticks below current in descending order
            initialized_ticks.retain(|t| t.tick_index <= current_tick);
            initialized_ticks.sort_by(|a, b| b.tick_index.cmp(&a.tick_index));
        } else {
            // Price increasing: walk ticks above current in ascending order
            initialized_ticks.retain(|t| t.tick_index > current_tick);
            initialized_ticks.sort_by(|a, b| a.tick_index.cmp(&b.tick_index));
        }

        // Safety limit to prevent infinite loops on bad data
        let max_steps = 50;

        for (steps, tick) in initialized_ticks.iter().enumerate() {
            if amount_remaining == 0 || steps >= max_steps {
                break;
            }

            let target_sqrt_price = tick_index_to_sqrt_price_x64(tick.tick_index)?;

            // Skip ticks that are in the wrong direction relative to current price
            if a_to_b && target_sqrt_price >= current_sqrt_price {
                // Cross the tick to update liquidity even if price hasn't moved
                current_liquidity =
                    (current_liquidity as i128).checked_sub(tick.liquidity_net)? as u128;
                continue;
            }
            if !a_to_b && target_sqrt_price <= current_sqrt_price {
                current_liquidity =
                    (current_liquidity as i128).checked_add(tick.liquidity_net)? as u128;
                continue;
            }

            if current_liquidity == 0 {
                // No liquidity in this range, jump to the tick boundary
                current_sqrt_price = target_sqrt_price;
                if a_to_b {
                    current_liquidity =
                        (current_liquidity as i128).checked_sub(tick.liquidity_net)? as u128;
                } else {
                    current_liquidity =
                        (current_liquidity as i128).checked_add(tick.liquidity_net)? as u128;
                }
                continue;
            }

            // Compute swap step within this constant-liquidity range
            let (amount_in, amount_out, next_sqrt_price) = compute_swap_step(
                current_sqrt_price,
                target_sqrt_price,
                current_liquidity,
                amount_remaining,
                a_to_b,
            )?;

            amount_remaining = amount_remaining.saturating_sub(amount_in);
            total_output += amount_out;
            current_sqrt_price = next_sqrt_price;

            // If we reached the tick boundary, cross it (adjust liquidity)
            if current_sqrt_price == target_sqrt_price {
                if a_to_b {
                    current_liquidity =
                        (current_liquidity as i128).checked_sub(tick.liquidity_net)? as u128;
                } else {
                    current_liquidity =
                        (current_liquidity as i128).checked_add(tick.liquidity_net)? as u128;
                }
            }
        }

        // Handle remaining amount in the last range (no more initialized ticks)
        if amount_remaining > 0 && current_liquidity > 0 {
            // Use sqrt_price limits as boundary (MIN_SQRT_PRICE / MAX_SQRT_PRICE)
            let limit_price = if a_to_b {
                MIN_SQRT_PRICE_X64
            } else {
                MAX_SQRT_PRICE_X64
            };
            if let Some((_, amt_out, _)) = compute_swap_step(
                current_sqrt_price,
                limit_price,
                current_liquidity,
                amount_remaining,
                a_to_b,
            ) {
                total_output += amt_out;
            }
        }

        if total_output > u64::MAX as u128 {
            return None;
        }
        Some(total_output as u64)
    }

    /// DLMM bin-by-bin swap simulation.
    /// Walks bins starting from active_id, consuming liquidity at each bin's price.
    ///
    /// Per-bin swap formulas (from DEX reference):
    ///   X->Y: out = (in_after_fee * price) >> 64;  max_in_after_fee = (amountY << 64) / price
    ///   Y->X: out = (in_after_fee << 64) / price;  max_in_after_fee = (amountX * price) >> 64
    ///
    /// Fee is applied as fee-on-amount: fee = ceil(amount * totalFee / (10^9 - totalFee))
    /// For simplicity we use base fee only: baseFee = baseFactor * binStep * 10
    /// The pool's fee_bps is used as an approximation (converted to the 10^9 scale).
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

        let swap_for_y = a_to_b; // X->Y when a_to_b
        let mut amount_left = input_amount as u128;
        let mut total_out: u128 = 0;
        let mut current_id = active_id;
        let q64: u128 = 1u128 << 64;

        // Convert fee_bps to the 10^9 scale used by DLMM.
        // fee_bps=1 means 0.01% = 100_000 in 10^9 scale.
        // totalFee in 10^9 scale: fee_bps * 100_000.
        // fee_on_amount = ceil(amount * totalFee / (10^9 - totalFee))
        let total_fee_rate = (self.fee_bps as u128) * 100_000;
        let fee_denom = 1_000_000_000u128.saturating_sub(total_fee_rate);
        if fee_denom == 0 {
            return None;
        }

        // Walk bins, max 200 as safety limit
        for _ in 0..200 {
            if amount_left == 0 {
                break;
            }

            // Find the bin in our cached arrays
            let array_idx = if current_id >= 0 {
                current_id as i64 / DLMM_MAX_BIN_PER_ARRAY as i64
            } else {
                (current_id as i64 - (DLMM_MAX_BIN_PER_ARRAY as i64 - 1))
                    / DLMM_MAX_BIN_PER_ARRAY as i64
            };
            let bin_offset =
                (current_id as i64 - array_idx * DLMM_MAX_BIN_PER_ARRAY as i64) as usize;

            let bin = bin_arrays
                .iter()
                .find(|a| a.index == array_idx)
                .and_then(|a| a.bins.get(bin_offset));

            let bin = match bin {
                Some(b) => b,
                None => break, // No more bin data available
            };

            if bin.price_q64 == 0 {
                break;
            }

            if swap_for_y {
                // X->Y: need Y liquidity in this bin
                if bin.amount_y == 0 {
                    current_id -= 1;
                    continue;
                }

                // Max input (after fee) that this bin can absorb:
                // max_in_after_fee = ceil((amountY << 64) / price)
                let max_in_after_fee = ((bin.amount_y as u128) << 64)
                    .checked_add(bin.price_q64 - 1)?
                    .checked_div(bin.price_q64)?;

                // Compute fee on amount_left to get amount_after_fee
                // fee = ceil(amount_left * total_fee_rate / fee_denom)
                // Simplification: amount_after_fee = amount_left - fee
                //                = amount_left - ceil(amount_left * total_fee_rate / fee_denom)
                // Equivalently: amount_after_fee = floor(amount_left * fee_denom / (fee_denom + total_fee_rate))
                // But the DLMM protocol computes fee-on-amount as:
                //   feeAmount = ceil(amountIn * totalFee / (10^9 - totalFee))
                //   amountInAfterFee = amountIn - feeAmount
                let fee_amount = ceil_div(amount_left * total_fee_rate, fee_denom);
                let amount_after_fee = amount_left.saturating_sub(fee_amount);

                if amount_after_fee >= max_in_after_fee {
                    // Consume entire bin
                    total_out += bin.amount_y as u128;
                    // Gross input consumed = max_in_after_fee + fee on that amount
                    // fee = ceil(max_in_after_fee * total_fee_rate / fee_denom)
                    let consumed_fee = ceil_div(max_in_after_fee * total_fee_rate, fee_denom);
                    let consumed = max_in_after_fee + consumed_fee;
                    amount_left = amount_left.saturating_sub(consumed);
                    current_id -= 1;
                } else {
                    // Partial fill: out = (amount_after_fee * price) >> 64
                    let out = amount_after_fee
                        .checked_mul(bin.price_q64)?
                        .checked_div(q64)?;
                    total_out += out;
                    amount_left = 0;
                }
            } else {
                // Y->X: need X liquidity in this bin
                if bin.amount_x == 0 {
                    current_id += 1;
                    continue;
                }

                // Max input (after fee) that this bin can absorb:
                // max_in_after_fee = ceil((amountX * price) >> 64)
                // = ceil(amountX * price / 2^64)
                let max_in_after_fee = ceil_div(
                    (bin.amount_x as u128).checked_mul(bin.price_q64)?,
                    q64,
                );

                let fee_amount = ceil_div(amount_left * total_fee_rate, fee_denom);
                let amount_after_fee = amount_left.saturating_sub(fee_amount);

                if amount_after_fee >= max_in_after_fee {
                    // Consume entire bin
                    total_out += bin.amount_x as u128;
                    let consumed_fee = ceil_div(max_in_after_fee * total_fee_rate, fee_denom);
                    let consumed = max_in_after_fee + consumed_fee;
                    amount_left = amount_left.saturating_sub(consumed);
                    current_id += 1;
                } else {
                    // Partial fill: out = (amount_after_fee << 64) / price
                    let out = (amount_after_fee << 64).checked_div(bin.price_q64)?;
                    total_out += out;
                    amount_left = 0;
                }
            }
        }

        if total_out > u64::MAX as u128 {
            return None;
        }
        Some(total_out as u64)
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

/// Ceiling division: ceil(a / b). Returns 0 if b == 0.
#[inline]
fn ceil_div(a: u128, b: u128) -> u128 {
    if b == 0 {
        return 0;
    }
    a.div_ceil(b)
}

// ─── CLMM multi-tick helpers ────────────────────────────────────────────────

/// Minimum sqrt_price_x64 (tick = -443636).
const MIN_SQRT_PRICE_X64: u128 = 4295048016;
/// Maximum sqrt_price_x64 (tick = 443636).
const MAX_SQRT_PRICE_X64: u128 = 79226673515401279992447579055;

/// Compute swap amounts within a single constant-liquidity range.
///
/// Formulas (P = sqrt_price Q64.64, L = liquidity, Q = 2^64):
///   a_to_b (price decreasing, token A in, token B out):
///     max_amount_in = L * Q * (current_P - target_P) / (current_P * target_P)
///     amount_out = L * (current_P - new_P) / Q
///   b_to_a (price increasing, token B in, token A out):
///     max_amount_in = L * (target_P - current_P) / Q
///     amount_out = L * Q * (new_P - P) / (P * new_P)
///
/// Returns (amount_in_consumed, amount_out, next_sqrt_price).
fn compute_swap_step(
    current_sqrt_price: u128,
    target_sqrt_price: u128,
    liquidity: u128,
    amount_remaining: u128,
    a_to_b: bool,
) -> Option<(u128, u128, u128)> {
    let q: u128 = 1u128 << 64;

    if a_to_b {
        // Price decreasing. Token A in, Token B out.
        let price_diff = current_sqrt_price.checked_sub(target_sqrt_price)?;
        // max_in = L * price_diff / current_P * Q / target_P
        // Reorder to avoid overflow: (L * price_diff / current_P) * (Q / target_P)
        // But that loses precision. Better: L * (Q / target_P - Q / current_P)
        // = L * Q * (current_P - target_P) / (current_P * target_P)
        // To avoid overflow, compute in steps:
        // step1 = L * price_diff / current_P  (fits u128 for reasonable L)
        // max_in = step1 * Q / target_P
        let step1 = liquidity.checked_mul(price_diff)?.checked_div(current_sqrt_price)?;
        let max_in = step1.checked_mul(q)?.checked_div(target_sqrt_price)?;

        let (amount_in, next_price) = if amount_remaining >= max_in {
            (max_in, target_sqrt_price)
        } else {
            // Partial fill: new_P = L * P / (L + amount * P / Q)
            let amt_x_price = amount_remaining.checked_mul(current_sqrt_price)?.checked_div(q)?;
            let denom = liquidity.checked_add(amt_x_price)?;
            if denom == 0 {
                return None;
            }
            let new_price = liquidity.checked_mul(current_sqrt_price)?.checked_div(denom)?;
            (amount_remaining, new_price)
        };

        // amount_out = L * (current_P - new_P) / Q
        let out_price_diff = current_sqrt_price.checked_sub(next_price)?;
        let amount_out = liquidity.checked_mul(out_price_diff)?.checked_div(q)?;

        Some((amount_in, amount_out, next_price))
    } else {
        // Price increasing. Token B in, Token A out.
        let price_diff = target_sqrt_price.checked_sub(current_sqrt_price)?;
        // max_in = L * price_diff / Q
        let max_in = liquidity.checked_mul(price_diff)?.checked_div(q)?;

        let (amount_in, next_price) = if amount_remaining >= max_in {
            (max_in, target_sqrt_price)
        } else {
            // Partial fill: new_P = P + amount * Q / L
            let delta = amount_remaining.checked_mul(q)?.checked_div(liquidity)?;
            let new_price = current_sqrt_price.checked_add(delta)?;
            (amount_remaining, new_price)
        };

        // amount_out = L * Q * (new_P - P) / (P * new_P)
        // Split: (L * (new_P - P) / P) * (Q / new_P)
        let out_price_diff = next_price.checked_sub(current_sqrt_price)?;
        let numerator = liquidity.checked_mul(out_price_diff)?;
        let step1 = numerator.checked_div(current_sqrt_price)?;
        let amount_out = step1.checked_mul(q)?.checked_div(next_price)?;

        Some((amount_in, amount_out, next_price))
    }
}

/// Convert tick index to sqrt_price in Q64.64 format.
///
/// sqrt_price = 1.0001^(tick/2) * 2^64
///
/// Uses f64 for the off-chain quoter. This is acceptable because:
/// - f64 has ~15 significant digits, sufficient for route discovery
/// - The on-chain program uses exact integer math; we only need approximate prices
///   to decide whether to submit a bundle
/// - The key constraint (pitfall #15) about u128 math applies to the swap math
///   (L*P products), not to this conversion function
pub fn tick_index_to_sqrt_price_x64(tick: i32) -> Option<u128> {
    let abs_tick = tick.unsigned_abs();
    if abs_tick > 443636 {
        return None;
    }

    // 1.0001^(tick/2) = 1.00005^tick (since sqrt(1.0001) = 1.00005 approximately)
    // More precisely: sqrt(1.0001) = 1.000049998750...
    // For f64, using powi is sufficiently accurate for route discovery.
    let sqrt_price_f64 = 1.0001_f64.powi(tick / 2)
        * if tick % 2 != 0 {
            if tick > 0 { 1.0001_f64.sqrt() } else { 1.0 / 1.0001_f64.sqrt() }
        } else {
            1.0
        };

    let q64 = (1u128 << 64) as f64;
    let result = sqrt_price_f64 * q64;

    if result <= 0.0 || result >= u128::MAX as f64 {
        return None;
    }

    Some(result as u128)
}
