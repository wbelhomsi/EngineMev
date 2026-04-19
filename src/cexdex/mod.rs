//! CEX-DEX arbitrage module (Model A, SOL/USDC).
//!
//! Run via `cargo run --release --bin cexdex`.

pub mod config;
pub mod units;
pub mod inventory;
pub mod route;
pub mod detector;
pub mod simulator;
pub mod bundle;
pub mod geyser;
pub mod nonce;
pub mod stats;

pub use config::CexDexConfig;
pub use inventory::Inventory;
pub use nonce::{NonceInfo, NoncePool};
pub use route::{ArbDirection, CexDexRoute};
