use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::time::Duration;

/// Known DEX program IDs on Solana
pub mod programs {
    use super::*;

    pub fn raydium_amm() -> Pubkey {
        Pubkey::from_str("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8").unwrap()
    }

    pub fn raydium_clmm() -> Pubkey {
        Pubkey::from_str("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK").unwrap()
    }

    pub fn orca_whirlpool() -> Pubkey {
        Pubkey::from_str("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc").unwrap()
    }

    pub fn meteora_dlmm() -> Pubkey {
        Pubkey::from_str("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo").unwrap()
    }

    pub fn jupiter_v6() -> Pubkey {
        Pubkey::from_str("JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4").unwrap()
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
    /// Maximum number of hops in arb route
    pub max_hops: usize,
    /// How long to cache pool state before refreshing
    pub pool_state_ttl: Duration,
    /// Simulation mode — log opportunities without submitting
    pub dry_run: bool,
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

        let min_profit_lamports: u64 = std::env::var("MIN_PROFIT_LAMPORTS")
            .unwrap_or_else(|_| "100000".to_string()) // 0.0001 SOL
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
            max_hops,
            pool_state_ttl: Duration::from_millis(400), // ~1 slot
            dry_run: std::env::var("DRY_RUN")
                .unwrap_or_else(|_| "true".to_string())
                .parse()?,
        })
    }

    /// DEX program IDs we monitor in the mempool
    pub fn monitored_programs(&self) -> Vec<Pubkey> {
        vec![
            programs::raydium_amm(),
            programs::raydium_clmm(),
            programs::orca_whirlpool(),
            programs::meteora_dlmm(),
        ]
    }
}
