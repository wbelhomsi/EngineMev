pub mod stream;

// decoder.rs removed — no longer needed.
// Old approach: decode pending swap txs from Jito mempool (dead since March 2024).
// New approach: observe pool vault balance changes via Yellowstone Geyser.
// Swap detection happens by comparing old vs new vault balances in the state cache.

pub use stream::{GeyserStream, PoolStateChange};
