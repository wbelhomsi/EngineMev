//! Manifest CLOB market-making module.
//!
//! Architecture mirrors `src/cexdex/` but inverted: instead of reacting to
//! CEX-DEX divergence as a taker, we continuously post resting bids and asks
//! around a CEX reference mid and earn the spread when counterparties trade
//! through our quotes.
//!
//! Pipeline:
//!   Binance WS (mid) → Quoter (target bid/ask) → BookState (what's live)
//!                                                    ↓
//!                                               BatchUpdate IX (cancel+repost)
//!                                                    ↓
//!                                               Relay (Jito / public RPC)
//!
//! Fill detection: periodic Geyser poll of the market account detects
//! our resting orders disappearing. Inventory accounting updated on fills.
//! Hedging via Binance is a follow-up — the v1 validation just measures
//! whether orders land and fill at all.
//!
//! Halal posture: market making on spot pairs is textbook-permissible (no
//! riba, no gharar, no maysir). Caveat: only trade pairs where both base
//! and quote are halal-compliant instruments.

pub mod config;
pub mod quoter;
pub mod book_state;

pub use config::MmConfig;
pub use quoter::{QuoteDecision, Quoter, QuoterConfig};
pub use book_state::{BookState, LiveOrder, OrderSide};
