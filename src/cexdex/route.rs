//! CexDexRoute type — full impl in Task 6.
//! This file provides ArbDirection now so Inventory can compile.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Placeholder — full implementation in Task 6.
pub struct CexDexRoute;
