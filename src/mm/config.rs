//! Environment-variable driven config for the Manifest MM binary.

use anyhow::{Context, Result};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

/// Runtime configuration for the Manifest market-making bot.
#[derive(Debug, Clone)]
pub struct MmConfig {
    // ─── Identity ─────────────────────────────────────────────────────────
    pub searcher_private_key: String,
    pub rpc_url: String,
    pub geyser_grpc_url: String,
    pub geyser_auth_token: String,

    // ─── Target market ────────────────────────────────────────────────────
    /// Manifest market account pubkey.
    pub market: Pubkey,
    /// Base mint (e.g. SOL, jitoSOL).
    pub base_mint: Pubkey,
    /// Quote mint (e.g. USDC).
    pub quote_mint: Pubkey,
    /// Base token decimals (9 for SOL, 8 for most LSTs).
    pub base_decimals: u8,
    /// Quote token decimals (6 for USDC).
    pub quote_decimals: u8,
    /// Binance symbol to use as reference (e.g. "SOLUSDC").
    pub cex_reference_symbol: String,

    // ─── Quoter ───────────────────────────────────────────────────────────
    /// Half-spread as fraction of mid. Default 5 bps (0.0005).
    pub half_spread_frac: f64,
    /// Max inventory-driven skew as fraction of mid. Default 10 bps.
    pub max_skew_frac: f64,
    /// Target inventory ratio. Default 0.5.
    pub target_inventory_ratio: f64,
    /// Skew ratio window for clamping. Default 0.3.
    pub skew_ratio_window: f64,
    /// Absolute minimum half-spread. Default 2 bps.
    pub min_half_spread_frac: f64,
    /// Order size in base atoms.
    pub order_size_base_atoms: u64,

    // ─── Cycle control ────────────────────────────────────────────────────
    /// Quote refresh interval (ms). Default 500ms.
    pub requote_interval_ms: u64,
    /// Maximum CEX price age (ms) before we stop quoting. Default 500ms.
    pub cex_staleness_ms: u64,
    /// If mid moves more than this fraction since last quote, force a requote.
    /// Default 2 bps (0.0002).
    pub requote_threshold_frac: f64,

    // ─── Safety ───────────────────────────────────────────────────────────
    /// Dry-run mode: log quote decisions, don't submit on-chain IXs.
    pub dry_run: bool,
    /// Auto-shutdown after this many seconds (0 = forever).
    pub run_secs: u64,
    /// Where to write run artifacts.
    pub stats_path: String,
    /// Prometheus metrics port. 0 = disabled.
    pub metrics_port: u16,
}

impl MmConfig {
    /// Load from environment. Fails loudly on missing required fields.
    pub fn from_env() -> Result<Self> {
        let searcher_private_key = std::env::var("MM_SEARCHER_PRIVATE_KEY")
            .context("MM_SEARCHER_PRIVATE_KEY required")?;
        let rpc_url = std::env::var("RPC_URL").context("RPC_URL required")?;
        let geyser_grpc_url =
            std::env::var("GEYSER_GRPC_URL").context("GEYSER_GRPC_URL required")?;
        let geyser_auth_token = std::env::var("GEYSER_AUTH_TOKEN").unwrap_or_default();

        let market_str =
            std::env::var("MM_MARKET").context("MM_MARKET (Manifest market pubkey) required")?;
        let market = Pubkey::from_str(&market_str).context("MM_MARKET invalid pubkey")?;

        let base_mint_str =
            std::env::var("MM_BASE_MINT").context("MM_BASE_MINT required")?;
        let base_mint = Pubkey::from_str(&base_mint_str).context("MM_BASE_MINT invalid")?;

        let quote_mint_str = std::env::var("MM_QUOTE_MINT").unwrap_or_else(|_| {
            // Default to USDC mainnet mint.
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string()
        });
        let quote_mint = Pubkey::from_str(&quote_mint_str).context("MM_QUOTE_MINT invalid")?;

        let base_decimals: u8 = std::env::var("MM_BASE_DECIMALS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(9);
        let quote_decimals: u8 = std::env::var("MM_QUOTE_DECIMALS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(6);
        let cex_reference_symbol =
            std::env::var("MM_CEX_REFERENCE_SYMBOL").unwrap_or_else(|_| "SOLUSDC".to_string());

        let half_spread_frac: f64 = env_float("MM_HALF_SPREAD_FRAC", 0.0005);
        let max_skew_frac: f64 = env_float("MM_MAX_SKEW_FRAC", 0.001);
        let target_inventory_ratio: f64 = env_float("MM_TARGET_INVENTORY_RATIO", 0.5);
        let skew_ratio_window: f64 = env_float("MM_SKEW_RATIO_WINDOW", 0.3);
        let min_half_spread_frac: f64 = env_float("MM_MIN_HALF_SPREAD_FRAC", 0.0002);
        let order_size_base_atoms: u64 = std::env::var("MM_ORDER_SIZE_BASE_ATOMS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100_000_000); // 0.1 SOL if base=SOL

        let requote_interval_ms: u64 = std::env::var("MM_REQUOTE_INTERVAL_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(500);
        let cex_staleness_ms: u64 = std::env::var("MM_CEX_STALENESS_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(500);
        let requote_threshold_frac: f64 = env_float("MM_REQUOTE_THRESHOLD_FRAC", 0.0002);

        let dry_run = std::env::var("MM_DRY_RUN")
            .unwrap_or_else(|_| "true".to_string())
            .to_lowercase()
            == "true";
        let run_secs: u64 = std::env::var("MM_RUN_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let stats_path =
            std::env::var("MM_STATS_PATH").unwrap_or_else(|_| "/tmp/manifest_mm".to_string());
        let metrics_port: u16 = std::env::var("MM_METRICS_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        // Sanity checks.
        if half_spread_frac <= 0.0 {
            anyhow::bail!("MM_HALF_SPREAD_FRAC must be positive");
        }
        if !(0.0..=1.0).contains(&target_inventory_ratio) {
            anyhow::bail!("MM_TARGET_INVENTORY_RATIO must be in [0, 1]");
        }
        if order_size_base_atoms == 0 {
            anyhow::bail!("MM_ORDER_SIZE_BASE_ATOMS must be positive");
        }

        Ok(Self {
            searcher_private_key,
            rpc_url,
            geyser_grpc_url,
            geyser_auth_token,
            market,
            base_mint,
            quote_mint,
            base_decimals,
            quote_decimals,
            cex_reference_symbol,
            half_spread_frac,
            max_skew_frac,
            target_inventory_ratio,
            skew_ratio_window,
            min_half_spread_frac,
            order_size_base_atoms,
            requote_interval_ms,
            cex_staleness_ms,
            requote_threshold_frac,
            dry_run,
            run_secs,
            stats_path,
            metrics_port,
        })
    }
}

fn env_float(key: &str, default: f64) -> f64 {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_float_returns_default_on_unset() {
        // Use a unique name to avoid env pollution.
        let v = env_float("MM_TEST_FAKE_NOT_SET_abc", 1.5);
        assert_eq!(v, 1.5);
    }
}
