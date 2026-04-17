//! CexDexRoute: a single-leg swap (USDC→SOL or SOL→USDC) on one pool.
//!
//! Unlike the main engine's `ArbRoute` which is circular (SOL→...→SOL),
//! this is unit-mismatched: input is one token, output is another.
//! Profit is calculated in USD via CEX prices, not by atom subtraction.

use solana_sdk::pubkey::Pubkey;

use crate::router::pool::DexType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArbDirection {
    /// DEX is cheap: we buy SOL on-chain with USDC.
    BuyOnDex,
    /// DEX is expensive: we sell SOL on-chain for USDC.
    SellOnDex,
}

impl ArbDirection {
    pub fn label(&self) -> &'static str {
        match self {
            ArbDirection::BuyOnDex => "buy_on_dex",
            ArbDirection::SellOnDex => "sell_on_dex",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CexDexRoute {
    pub pool_address: Pubkey,
    pub dex_type: DexType,
    pub direction: ArbDirection,
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    pub input_amount: u64,        // atoms of input_mint
    pub expected_output: u64,     // atoms of output_mint (at current pool state)
    pub cex_bid_at_detection: f64,
    pub cex_ask_at_detection: f64,
    pub expected_profit_usd: f64, // gross, before tip
    pub observed_slot: u64,
}

impl CexDexRoute {
    pub fn cex_mid(&self) -> f64 {
        (self.cex_bid_at_detection + self.cex_ask_at_detection) / 2.0
    }
}
