use crate::addresses;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::sync::LazyLock;
use std::time::Duration;

/// Redact API keys from URLs and error messages for safe logging.
/// Replaces `api-key=...` and `api_key=...` query params with `REDACTED`.
pub fn redact_url(s: &str) -> String {
    let mut result = s.to_string();
    // Redact api-key=VALUE patterns (stops at & or end of string)
    for pattern in ["api-key=", "api_key=", "x-token=", "token="] {
        if let Some(start) = result.find(pattern) {
            let value_start = start + pattern.len();
            let value_end = result[value_start..].find('&')
                .map(|i| value_start + i)
                .unwrap_or(result.len());
            result.replace_range(value_start..value_end, "REDACTED");
        }
    }
    result
}

/// Re-export program IDs for backward compatibility.
/// All addresses live in `crate::addresses`; these thin wrappers ease migration.
pub mod programs {
    use crate::addresses;
    use solana_sdk::pubkey::Pubkey;

    pub fn raydium_amm() -> Pubkey { addresses::RAYDIUM_AMM }
    pub fn raydium_clmm() -> Pubkey { addresses::RAYDIUM_CLMM }
    pub fn orca_whirlpool() -> Pubkey { addresses::ORCA_WHIRLPOOL }
    pub fn meteora_dlmm() -> Pubkey { addresses::METEORA_DLMM }
    pub fn raydium_cp() -> Pubkey { addresses::RAYDIUM_CP }
    pub fn meteora_damm_v2() -> Pubkey { addresses::METEORA_DAMM_V2 }
    pub fn sanctum_s_controller() -> Pubkey { addresses::SANCTUM_S_CONTROLLER }
    pub fn sanctum_pricing() -> Pubkey { addresses::SANCTUM_PRICING }
    pub fn phoenix_v1() -> Pubkey { addresses::PHOENIX_V1 }
    pub fn manifest() -> Pubkey { addresses::MANIFEST }
}

// ─── Static mints and calculators (parsed once) ────────────────────────────

static JITOSOL_MINT: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap()
});
static MSOL_MINT: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So").unwrap()
});
static BSOL_MINT: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("bSo13r4TkiE4KumL71LsHTPpL2euBYLFx6h9HP3piy1").unwrap()
});
static SOL_MINT: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap()
});
static SPL_STAKE_POOL_CALC: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("sp1V4h2gWorkGhVcazBc22Hfo2f5sd7jcjT4EDPrWFF").unwrap()
});
static MARINADE_CALC: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("mare3SCyfZkAndpBRBeonETmkCCB3TJTTrz8ZN2dnhP").unwrap()
});

// ─── Sanctum static addresses (verified on-chain 2026-04-03) ──────────────

static WSOL_CALCULATOR: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("wsoGmxQLSvwWpuaidCApxN5kEowLe2HLQLJhCQnj4bE").unwrap()
});

// SPL Stake Pool Calculator accounts
static SPL_CALC_STATE: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("7orJ4kDhn1Ewp54j29tBzUWDFGhyimhYi7sxybZcphHd").unwrap()
});
static SPL_STAKE_POOL_PROGRAM: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("SPoo1Ku8WFXoNDMHPsrGSTSG1Y47rzgn41SLUNakuHy").unwrap()
});
static SPL_STAKE_POOL_PROG_DATA: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("EmiU8AQkB2sswTxVB6aCmsAJftoowZGGDXuytm6X65R3").unwrap()
});
static JITO_STAKE_POOL: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("Jito4APyf642JPZPx3hGc6WWJ8zPKtRbRs4P815Awbb").unwrap()
});
static BLAZE_STAKE_POOL: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("stk9ApL5HeVAwPLr3TLhDXdZS8ptVu7zp6ov8HFDuMi").unwrap()
});

// Marinade Calculator accounts
static MARINADE_CALC_STATE: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("FMbUjYFtqgm4Zfpg7MguZp33RQ3tvkd22NgaCCAs3M6E").unwrap()
});
static MARINADE_STATE: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("8szGkuLTAux9XMgZ2vtY39jVSowEcpBfFfD8hXSEqdGC").unwrap()
});
static MARINADE_PROGRAM: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("MarBmsSgKXdrN1egZf5sqe1TMai9K1rChYNDJgjq7aD").unwrap()
});
static MARINADE_PROG_DATA: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("4PQH9YmfuKrVyZaibkLYpJZPv2FPaybhq2GAuBcWMSBf").unwrap()
});

// Pricing program state
static SANCTUM_PRICING_STATE: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("4T9YzXnmQFMyYi2nrxyXjhtUANavmCkxGCsU3GKaNjwT").unwrap()
});

/// Sanctum pricing program state account.
pub fn sanctum_pricing_state() -> Pubkey { *SANCTUM_PRICING_STATE }

/// Jito SPL Stake Pool account (for LST rate fetch).
pub fn jito_stake_pool() -> Pubkey { *JITO_STAKE_POOL }
/// Blaze SPL Stake Pool account (for LST rate fetch).
pub fn blaze_stake_pool() -> Pubkey { *BLAZE_STAKE_POOL }
/// Marinade state account (for LST rate fetch).
pub fn marinade_state() -> Pubkey { *MARINADE_STATE }

/// Returns (calculator_program, remaining_accounts, calc_accs_count) for a given LST mint.
/// The remaining_accounts are the suffix accounts after the calculator program.
pub fn sanctum_calculator_accounts(mint: &Pubkey) -> (Pubkey, Vec<Pubkey>, u8) {
    if *mint == sol_mint() {
        // wSOL: just the calculator program, no extra accounts
        (*WSOL_CALCULATOR, vec![], 1)
    } else if *mint == *JITOSOL_MINT {
        // jitoSOL: SPL Stake Pool calculator
        (*SPL_STAKE_POOL_CALC, vec![
            *SPL_CALC_STATE,
            *JITO_STAKE_POOL,
            *SPL_STAKE_POOL_PROGRAM,
            *SPL_STAKE_POOL_PROG_DATA,
        ], 5)
    } else if *mint == *BSOL_MINT {
        // bSOL: SPL Stake Pool calculator (different pool)
        (*SPL_STAKE_POOL_CALC, vec![
            *SPL_CALC_STATE,
            *BLAZE_STAKE_POOL,
            *SPL_STAKE_POOL_PROGRAM,
            *SPL_STAKE_POOL_PROG_DATA,
        ], 5)
    } else if *mint == *MSOL_MINT {
        // mSOL: Marinade calculator
        (*MARINADE_CALC, vec![
            *MARINADE_CALC_STATE,
            *MARINADE_STATE,
            *MARINADE_PROGRAM,
            *MARINADE_PROG_DATA,
        ], 5)
    } else {
        // Unknown LST: fallback to wSOL calculator (will fail on-chain but safe)
        (*WSOL_CALCULATOR, vec![], 1)
    }
}

/// Supported LST mints and their human-readable names.
pub fn lst_mints() -> Vec<(Pubkey, &'static str)> {
    vec![
        (*JITOSOL_MINT, "jitoSOL"),
        (*MSOL_MINT, "mSOL"),
        (*BSOL_MINT, "bSOL"),
    ]
}

/// Native SOL mint (wrapped SOL).
pub fn sol_mint() -> Pubkey {
    *SOL_MINT
}

/// Map an LST mint to its Sanctum SOL Value Calculator program.
/// Returns None for unknown mints.
pub fn sanctum_sol_value_calculator(mint: &Pubkey) -> Option<Pubkey> {
    if *mint == *JITOSOL_MINT || *mint == *BSOL_MINT {
        Some(*SPL_STAKE_POOL_CALC)
    } else if *mint == *MSOL_MINT {
        Some(*MARINADE_CALC)
    } else {
        None
    }
}

#[derive(Debug, Clone)]
pub struct BotConfig {
    /// Jito block engine endpoint
    pub jito_block_engine_url: String,
    /// Jito gRPC auth keypair path
    pub jito_auth_keypair_path: String,
    /// Yellowstone gRPC endpoint for Geyser streaming
    pub geyser_grpc_url: String,
    /// Geyser auth token
    pub geyser_auth_token: String,
    /// RPC endpoint for state queries
    pub rpc_url: String,
    /// Searcher/signer keypair path
    pub searcher_keypair_path: String,

    /// Relay endpoints for multi-relay fan-out
    pub relay_endpoints: RelayEndpoints,

    /// Tip as fraction of estimated profit (0.0 - 1.0)
    pub tip_fraction: f64,
    /// Minimum profit in lamports to submit a bundle
    pub min_profit_lamports: u64,
    /// Minimum tip in lamports (floor for Jito auction competitiveness)
    pub min_tip_lamports: u64,
    /// Maximum number of hops in arb route
    pub max_hops: usize,
    /// How long to cache pool state before refreshing
    pub pool_state_ttl: Duration,
    /// Slippage tolerance on estimated profit (0.0 - 1.0). Default 0.25 = 25%.
    /// Plan as if we realize (1 - slippage) of gross profit for tipping and min_amount_out.
    pub slippage_tolerance: f64,
    /// Simulation mode — log opportunities without submitting
    pub dry_run: bool,
    /// Enable LST rate arbitrage (jitoSOL, mSOL, bSOL cross-DEX + Sanctum)
    pub lst_arb_enabled: bool,
    /// Minimum spread in basis points for LST arb routes
    pub lst_min_spread_bps: u64,
    /// Optional arb-guard program ID for on-chain profit verification
    pub arb_guard_program_id: Option<Pubkey>,
    /// Port for Prometheus /metrics HTTP endpoint (disabled if None)
    pub metrics_port: Option<u16>,
    /// OTLP gRPC endpoint for tracing spans (disabled if None)
    pub otlp_endpoint: Option<String>,
    /// Service name reported in OTLP traces
    pub otlp_service_name: String,
}

#[derive(Debug, Clone)]
pub struct RelayEndpoints {
    pub jito: String,
    pub nozomi: Option<String>,
    pub bloxroute: Option<String>,
    pub astralane: Option<String>,
    pub zeroslot: Option<String>,
}

impl BotConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        dotenv::dotenv().ok();

        let tip_fraction: f64 = std::env::var("TIP_FRACTION")
            .unwrap_or_else(|_| "0.50".to_string())
            .parse()?;

        anyhow::ensure!(
            tip_fraction > 0.0 && tip_fraction < 1.0,
            "TIP_FRACTION must be between 0 and 1 (exclusive), got {}",
            tip_fraction
        );

        let min_profit_lamports: u64 = std::env::var("MIN_PROFIT_LAMPORTS")
            .unwrap_or_else(|_| "100000".to_string()) // 0.0001 SOL
            .parse()?;

        let min_tip_lamports: u64 = std::env::var("MIN_TIP_LAMPORTS")
            .unwrap_or_else(|_| "50000".to_string()) // 0.00005 SOL
            .parse()?;

        let max_hops: usize = std::env::var("MAX_HOPS")
            .unwrap_or_else(|_| "3".to_string())
            .parse()?;

        Ok(Self {
            jito_block_engine_url: std::env::var("JITO_BLOCK_ENGINE_URL")
                .unwrap_or_else(|_| "https://mainnet.block-engine.jito.wtf".to_string()),
            jito_auth_keypair_path: std::env::var("JITO_AUTH_KEYPAIR")
                .unwrap_or_else(|_| "keypair.json".to_string()),
            geyser_grpc_url: std::env::var("GEYSER_GRPC_URL")
                .unwrap_or_else(|_| "http://localhost:10000".to_string()),
            geyser_auth_token: std::env::var("GEYSER_AUTH_TOKEN")
                .unwrap_or_default(),
            rpc_url: std::env::var("RPC_URL")
                .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string()),
            searcher_keypair_path: std::env::var("SEARCHER_KEYPAIR")
                .unwrap_or_else(|_| "searcher.json".to_string()),
            relay_endpoints: RelayEndpoints {
                jito: std::env::var("JITO_RELAY_URL")
                    .unwrap_or_else(|_| "https://mainnet.block-engine.jito.wtf".to_string()),
                nozomi: std::env::var("NOZOMI_RELAY_URL").ok(),
                bloxroute: std::env::var("BLOXROUTE_RELAY_URL").ok(),
                astralane: std::env::var("ASTRALANE_RELAY_URL").ok(),
                zeroslot: std::env::var("ZEROSLOT_RELAY_URL").ok(),
            },
            tip_fraction,
            min_profit_lamports,
            min_tip_lamports,
            max_hops,
            pool_state_ttl: Duration::from_secs(2), // 2s — tight TTL for co-located Frankfurt server
            slippage_tolerance: std::env::var("SLIPPAGE_TOLERANCE")
                .unwrap_or_else(|_| "0.25".to_string())
                .parse()?,
            dry_run: std::env::var("DRY_RUN")
                .unwrap_or_else(|_| "true".to_string())
                .parse()?,
            lst_arb_enabled: std::env::var("LST_ARB_ENABLED")
                .unwrap_or_else(|_| "true".to_string())
                .parse()?,
            lst_min_spread_bps: std::env::var("LST_MIN_SPREAD_BPS")
                .unwrap_or_else(|_| "5".to_string())
                .parse()?,
            arb_guard_program_id: std::env::var("ARB_GUARD_PROGRAM_ID")
                .ok()
                .and_then(|s| Pubkey::from_str(&s).ok()),
            metrics_port: std::env::var("METRICS_PORT")
                .ok()
                .and_then(|v| v.parse().ok()),
            otlp_endpoint: std::env::var("OTLP_ENDPOINT").ok().filter(|s| !s.is_empty()),
            otlp_service_name: std::env::var("OTLP_SERVICE_NAME")
                .unwrap_or_else(|_| "mev-engine".to_string()),
        })
    }

    /// DEX program IDs we monitor in the mempool
    pub fn monitored_programs(&self) -> Vec<Pubkey> {
        let mut programs = vec![
            addresses::RAYDIUM_AMM,
            addresses::RAYDIUM_CLMM,
            addresses::RAYDIUM_CP,
            addresses::ORCA_WHIRLPOOL,
            addresses::METEORA_DLMM,
            addresses::METEORA_DAMM_V2,
            addresses::PHOENIX_V1,
            addresses::MANIFEST,
            addresses::PUMPSWAP,
        ];
        if self.lst_arb_enabled {
            programs.push(addresses::SANCTUM_S_CONTROLLER);
        }
        programs
    }
}
