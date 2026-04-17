//! CEX-DEX configuration loaded from environment variables.
//!
//! All env vars prefixed `CEXDEX_` to avoid collision with the main engine.

use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::time::Duration;

use crate::router::pool::DexType;

#[derive(Debug, Clone)]
pub struct CexDexConfig {
    // Wallet (separate from main engine)
    pub searcher_keypair_path: String,
    pub searcher_private_key: Option<String>,

    // Solana
    pub rpc_url: String,
    pub geyser_grpc_url: String,
    pub geyser_auth_token: String,

    // Binance
    pub binance_ws_url: String,
    pub cex_staleness_ms: u64,

    // Pools to monitor (DexType + Pubkey pairs)
    pub pools: Vec<(DexType, Pubkey)>,

    // Strategy
    pub min_spread_bps: u64,
    pub min_profit_usd: f64,
    pub max_trade_size_sol: f64,
    pub max_position_fraction: f64,
    pub dedup_window_ms: u64,
    pub global_submit_cooldown_ms: u64,

    // Inventory gates
    pub hard_cap_ratio: f64,
    pub preferred_low: f64,
    pub preferred_high: f64,
    pub skewed_profit_multiplier: f64,

    // Slippage
    pub slippage_tolerance: f64,

    // Fraction of slippage-adjusted profit offered as tip (separate from main engine's TIP_FRACTION)
    pub tip_fraction: f64,

    /// Nonce accounts for multi-relay non-equivocation. Parsed from
    /// CEXDEX_SEARCHER_NONCE_ACCOUNTS (comma-separated pubkeys).
    /// Empty vec = nonce-less mode (backward-compat, Jito-only).
    pub nonce_accounts: Vec<Pubkey>,

    /// Per-relay tip fractions. Key = relay name (e.g. "jito", "astralane").
    /// Falls back to `tip_fraction` (CEXDEX_TIP_FRACTION) if a specific
    /// relay's env var isn't set.
    pub tip_fractions: std::collections::HashMap<String, f64>,

    // Safety
    pub dry_run: bool,
    pub pool_state_ttl: Duration,

    // Relays (reuses main engine env vars)
    pub jito_block_engine_url: String,
    pub astralane_relay_url: Option<String>,
    pub astralane_api_key: Option<String>,

    // Arb-guard program (optional — single-leg may not need it)
    pub arb_guard_program_id: Option<Pubkey>,

    // Metrics
    pub metrics_port: Option<u16>,
}

impl CexDexConfig {
    pub fn from_env() -> Result<Self> {
        dotenv::dotenv().ok();

        let searcher_keypair_path = std::env::var("CEXDEX_SEARCHER_KEYPAIR")
            .unwrap_or_else(|_| "cexdex-searcher.json".to_string());
        let searcher_private_key = std::env::var("CEXDEX_SEARCHER_PRIVATE_KEY").ok();

        let rpc_url = std::env::var("RPC_URL")
            .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());
        let geyser_grpc_url = std::env::var("GEYSER_GRPC_URL")
            .unwrap_or_else(|_| "http://localhost:10000".to_string());
        let geyser_auth_token = std::env::var("GEYSER_AUTH_TOKEN").unwrap_or_default();

        let binance_ws_url = std::env::var("CEXDEX_BINANCE_WS_URL")
            .unwrap_or_else(|_| "wss://stream.binance.com:9443/ws".to_string());
        let cex_staleness_ms: u64 = std::env::var("CEXDEX_CEX_STALENESS_MS")
            .unwrap_or_else(|_| "500".to_string())
            .parse()?;

        // Format: "RaydiumCp:<pubkey>,Orca:<pubkey>,..."
        let pools_raw = std::env::var("CEXDEX_POOLS").unwrap_or_default();
        let mut pools: Vec<(DexType, Pubkey)> = Vec::new();
        for entry in pools_raw.split(',').filter(|s| !s.is_empty()) {
            let (dex_str, pk_str) = entry.split_once(':')
                .ok_or_else(|| anyhow::anyhow!("Invalid CEXDEX_POOLS entry: {}", entry))?;
            let dex_type = match dex_str.trim() {
                "RaydiumAmm" => DexType::RaydiumAmm,
                "RaydiumCp" => DexType::RaydiumCp,
                "RaydiumClmm" => DexType::RaydiumClmm,
                "Orca" | "OrcaWhirlpool" => DexType::OrcaWhirlpool,
                "MeteoraDlmm" => DexType::MeteoraDlmm,
                "MeteoraDammV2" => DexType::MeteoraDammV2,
                other => anyhow::bail!("Unsupported DexType for CEX-DEX: {}", other),
            };
            let pubkey = Pubkey::from_str(pk_str.trim())
                .map_err(|e| anyhow::anyhow!("Invalid pubkey '{}': {}", pk_str, e))?;
            pools.push((dex_type, pubkey));
        }

        let min_spread_bps: u64 = std::env::var("CEXDEX_MIN_SPREAD_BPS")
            .unwrap_or_else(|_| "15".to_string()).parse()?;
        let min_profit_usd: f64 = std::env::var("CEXDEX_MIN_PROFIT_USD")
            .unwrap_or_else(|_| "0.10".to_string()).parse()?;
        anyhow::ensure!(
            min_profit_usd > 0.0,
            "CEXDEX_MIN_PROFIT_USD must be strictly positive (got {}) — never submit a non-profitable bundle",
            min_profit_usd
        );
        let max_trade_size_sol: f64 = std::env::var("CEXDEX_MAX_TRADE_SIZE_SOL")
            .unwrap_or_else(|_| "10.0".to_string()).parse()?;

        let max_position_fraction: f64 = std::env::var("CEXDEX_MAX_POSITION_FRACTION")
            .unwrap_or_else(|_| "0.20".to_string()).parse()?;
        anyhow::ensure!(
            max_position_fraction > 0.0 && max_position_fraction <= 1.0,
            "CEXDEX_MAX_POSITION_FRACTION must be in (0, 1], got {}",
            max_position_fraction
        );

        let dedup_window_ms: u64 = std::env::var("CEXDEX_DEDUP_WINDOW_MS")
            .unwrap_or_else(|_| "500".to_string()).parse()?;
        let global_submit_cooldown_ms: u64 = std::env::var("CEXDEX_GLOBAL_SUBMIT_COOLDOWN_MS")
            .unwrap_or_else(|_| "1500".to_string()).parse()?;

        let hard_cap_ratio: f64 = std::env::var("CEXDEX_HARD_CAP_RATIO")
            .unwrap_or_else(|_| "0.80".to_string()).parse()?;
        let preferred_low: f64 = std::env::var("CEXDEX_PREFERRED_LOW")
            .unwrap_or_else(|_| "0.40".to_string()).parse()?;
        let preferred_high: f64 = std::env::var("CEXDEX_PREFERRED_HIGH")
            .unwrap_or_else(|_| "0.60".to_string()).parse()?;
        let skewed_profit_multiplier: f64 = std::env::var("CEXDEX_SKEWED_PROFIT_MULTIPLIER")
            .unwrap_or_else(|_| "2.0".to_string()).parse()?;

        let slippage_tolerance: f64 = std::env::var("CEXDEX_SLIPPAGE_TOLERANCE")
            .unwrap_or_else(|_| "0.25".to_string()).parse()?;

        let tip_fraction: f64 = std::env::var("CEXDEX_TIP_FRACTION")
            .unwrap_or_else(|_| "0.50".to_string()).parse()?;
        anyhow::ensure!(
            tip_fraction > 0.0 && tip_fraction < 1.0,
            "CEXDEX_TIP_FRACTION must be between 0 and 1 (exclusive), got {}",
            tip_fraction
        );

        // Nonce accounts (optional for backward-compat — empty vec = nonce-less mode).
        let nonce_accounts: Vec<Pubkey> = std::env::var("CEXDEX_SEARCHER_NONCE_ACCOUNTS")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .map(|s| Pubkey::from_str(s.trim()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("Invalid CEXDEX_SEARCHER_NONCE_ACCOUNTS pubkey: {}", e))?;

        // Per-relay tip fractions. Uses tip_fraction (parsed above) as the default.
        let mut tip_fractions: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        for (relay_name, env_key) in [
            ("jito", "CEXDEX_TIP_FRACTION_JITO"),
            ("astralane", "CEXDEX_TIP_FRACTION_ASTRALANE"),
            ("nozomi", "CEXDEX_TIP_FRACTION_NOZOMI"),
            ("bloxroute", "CEXDEX_TIP_FRACTION_BLOXROUTE"),
            ("zeroslot", "CEXDEX_TIP_FRACTION_ZEROSLOT"),
        ] {
            let f: f64 = std::env::var(env_key).ok()
                .map(|s| s.parse::<f64>())
                .transpose()?
                .unwrap_or(tip_fraction);
            anyhow::ensure!(
                f > 0.0 && f < 1.0,
                "{} must be between 0 and 1 (exclusive), got {}",
                env_key, f,
            );
            tip_fractions.insert(relay_name.to_string(), f);
        }
        anyhow::ensure!(
            !tip_fractions.is_empty(),
            "tip_fractions map must have at least one entry — aborting to avoid divide-by-zero"
        );

        let dry_run = std::env::var("CEXDEX_DRY_RUN")
            .unwrap_or_else(|_| "true".to_string()).parse()?;

        let pool_state_ttl = Duration::from_secs(
            std::env::var("CEXDEX_POOL_TTL_SECS")
                .unwrap_or_else(|_| "5".to_string()).parse()?,
        );

        let jito_block_engine_url = std::env::var("JITO_BLOCK_ENGINE_URL")
            .unwrap_or_else(|_| "https://mainnet.block-engine.jito.wtf".to_string());
        let astralane_relay_url = std::env::var("ASTRALANE_RELAY_URL").ok();
        let astralane_api_key = std::env::var("ASTRALANE_API_KEY").ok();

        let arb_guard_program_id = std::env::var("ARB_GUARD_PROGRAM_ID")
            .ok()
            .and_then(|s| Pubkey::from_str(&s).ok());

        let metrics_port = std::env::var("CEXDEX_METRICS_PORT").ok()
            .and_then(|s| s.parse().ok());

        Ok(Self {
            searcher_keypair_path,
            searcher_private_key,
            rpc_url,
            geyser_grpc_url,
            geyser_auth_token,
            binance_ws_url,
            cex_staleness_ms,
            pools,
            min_spread_bps,
            min_profit_usd,
            max_trade_size_sol,
            max_position_fraction,
            dedup_window_ms,
            global_submit_cooldown_ms,
            hard_cap_ratio,
            preferred_low,
            preferred_high,
            skewed_profit_multiplier,
            slippage_tolerance,
            tip_fraction,
            nonce_accounts,
            tip_fractions,
            dry_run,
            pool_state_ttl,
            jito_block_engine_url,
            astralane_relay_url,
            astralane_api_key,
            arb_guard_program_id,
            metrics_port,
        })
    }
}
