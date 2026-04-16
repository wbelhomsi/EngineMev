//! CEX-DEX arbitrage binary.
//!
//! Run: `cargo run --release --bin cexdex`

use anyhow::Result;
use solana_mev_bot::cexdex::CexDexConfig;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .json()
        .init();

    info!("=== CEX-DEX Arbitrage Engine (Model A, SOL/USDC) ===");
    let config = CexDexConfig::from_env()?;
    info!(
        "Config: min_spread={}bps, min_profit=${:.2}, max_trade={} SOL, dry_run={}",
        config.min_spread_bps, config.min_profit_usd, config.max_trade_size_sol, config.dry_run,
    );
    info!("Monitoring {} pools", config.pools.len());

    // Full pipeline wired up in Task 11.
    anyhow::bail!("CEX-DEX binary: pipeline not yet wired (see Task 11)");
}
