use anyhow::Result;
use crossbeam_channel::Sender;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{info, warn, debug};
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof,
    CommitmentLevel, SubscribeRequest, SubscribeRequestFilterAccounts,
};

use crate::config::BotConfig;
use crate::state::StateCache;

/// Notification that a pool's on-chain state was updated.
/// The actual state is already in the StateCache — this is just a signal
/// telling the router which pool to re-evaluate.
#[derive(Debug, Clone)]
pub struct PoolStateChange {
    pub pool_address: Pubkey,
    pub slot: u64,
}

/// Streams pool state changes via Yellowstone gRPC Geyser plugin.
///
/// Architecture (post Jito mempool shutdown, March 2024):
///
/// OLD (dead): Jito subscribe_mempool → see pending tx → backrun in same bundle
/// NEW (current): Geyser account stream → see vault balance change → submit arb next slot
///
/// Yellowstone gRPC streams account updates directly from validator memory
/// at sub-50ms latency vs 100-300ms for standard WebSocket.
///
/// Flow:
/// 1. Subscribe to token vault accounts owned by target DEX programs
/// 2. When vault balances change (someone swapped), emit PoolStateChange
/// 3. Downstream router detects price dislocation across DEXes
/// 4. Bundle submitted for next slot via multi-relay fan-out
pub struct GeyserStream {
    config: Arc<BotConfig>,
    state_cache: StateCache,
    http_client: reqwest::Client,
}

impl GeyserStream {
    pub fn new(config: Arc<BotConfig>, state_cache: StateCache, http_client: reqwest::Client) -> Self {
        Self {
            config,
            state_cache,
            http_client,
        }
    }

    /// Start streaming pool state changes via Yellowstone gRPC.
    pub async fn start(
        &self,
        tx_sender: Sender<PoolStateChange>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Result<()> {
        let programs = self.config.monitored_programs();
        info!(
            "Starting Geyser stream, monitoring {} DEX programs",
            programs.len()
        );

        // Connect to Yellowstone gRPC endpoint with TLS (required for Helius LaserStream)
        let mut client = GeyserGrpcClient::build_from_shared(
            self.config.geyser_grpc_url.clone(),
        )?
        .x_token(if self.config.geyser_auth_token.is_empty() {
            None
        } else {
            Some(self.config.geyser_auth_token.clone())
        })?
        .tls_config(yellowstone_grpc_client::ClientTlsConfig::new().with_native_roots())?
        .connect()
        .await?;

        info!("Connected to Geyser gRPC at {}", crate::config::redact_url(&self.config.geyser_grpc_url));

        // Build subscription: watch all accounts owned by target DEX programs.
        // When a swap happens, the pool's token vault accounts get updated.
        let mut accounts_filter: HashMap<String, SubscribeRequestFilterAccounts> = HashMap::new();

        for (i, program_id) in programs.iter().enumerate() {
            accounts_filter.insert(
                format!("dex_{}", i),
                SubscribeRequestFilterAccounts {
                    account: vec![],
                    owner: vec![program_id.to_string()],
                    filters: vec![],
                    nonempty_txn_signature: None,
                },
            );
        }

        let subscribe_request = SubscribeRequest {
            accounts: accounts_filter,
            commitment: Some(CommitmentLevel::Processed as i32),
            ..Default::default()
        };

        let (_, mut stream) = client.subscribe_with_request(Some(subscribe_request)).await?;

        info!("Geyser subscription active, waiting for account updates...");

        // Main event loop
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Geyser stream: shutdown signal received");
                        break;
                    }
                }
                msg = Self::next_message(&mut stream) => {
                    match msg {
                        Some(update) => {
                            self.process_update(update, &tx_sender);
                        }
                        None => {
                            warn!("Geyser stream ended, needs reconnect");
                            break; // Caller should implement reconnect loop
                        }
                    }
                }
            }
        }

        info!("Geyser stream loop exited");
        Ok(())
    }

    /// Extract next message from the gRPC stream.
    async fn next_message(
        stream: &mut (impl tokio_stream::Stream<Item = Result<yellowstone_grpc_proto::prelude::SubscribeUpdate, yellowstone_grpc_proto::tonic::Status>> + Unpin),
    ) -> Option<yellowstone_grpc_proto::prelude::SubscribeUpdate> {
        use tokio_stream::StreamExt;
        stream.next().await?.ok()
    }

    /// Process a Geyser account update.
    ///
    /// Identifies DEX by account data size, dispatches to per-DEX parser,
    /// updates the StateCache, and for Raydium AMM/CP pools triggers an async
    /// vault balance fetch (since those pools don't embed reserves).
    fn process_update(
        &self,
        update: yellowstone_grpc_proto::prelude::SubscribeUpdate,
        tx_sender: &Sender<PoolStateChange>,
    ) {
        let Some(update_oneof) = update.update_oneof else {
            return;
        };

        match update_oneof {
            UpdateOneof::Account(account_update) => {
                let Some(account_info) = account_update.account else {
                    return;
                };

                let slot = account_update.slot;
                let data = &account_info.data;

                let pubkey_bytes: [u8; 32] = match account_info.pubkey.try_into() {
                    Ok(b) => b,
                    Err(_) => return,
                };
                let pool_address = Pubkey::new_from_array(pubkey_bytes);

                // Route to per-DEX parser based on account data size
                let parsed = match data.len() {
                    653 => parse_orca_whirlpool(&pool_address, data, slot).map(|p| (p, None)),
                    1560 => parse_raydium_clmm(&pool_address, data, slot).map(|p| (p, None)),
                    904 => parse_meteora_dlmm(&pool_address, data, slot).map(|p| (p, None)),
                    1112 => parse_meteora_damm_v2(&pool_address, data, slot).map(|p| (p, None)),
                    752 => parse_raydium_amm_v4(&pool_address, data, slot)
                        .map(|(p, vaults)| (p, Some(vaults))),
                    637 => parse_raydium_cp(&pool_address, data, slot)
                        .map(|(p, vaults)| (p, Some(vaults))),
                    _ => {
                        // Variable-size accounts: try orderbook DEX parsers
                        try_parse_orderbook(&pool_address, data, slot).map(|p| (p, None))
                    }
                };

                let Some((pool_state, vault_info)) = parsed else {
                    return;
                };

                // Update cache with parsed pool state
                let pool_mints = (pool_state.token_a_mint, pool_state.token_b_mint);
                self.state_cache.upsert(pool_address, pool_state);

                // Fetch token program for each mint (async, cached)
                for mint in [pool_mints.0, pool_mints.1] {
                    if self.state_cache.get_mint_program(&mint).is_none() {
                        let client = self.http_client.clone();
                        let url = self.config.rpc_url.clone();
                        let cache = self.state_cache.clone();
                        tokio::spawn(async move {
                            let _ = fetch_mint_program(&client, &url, &cache, &mint).await;
                        });
                    }
                }

                // For Category B (Raydium AMM/CP): trigger async vault balance fetch
                if let Some((vault_a, vault_b)) = vault_info {
                    let client = self.http_client.clone();
                    let url = self.config.rpc_url.clone();
                    let cache = self.state_cache.clone();
                    tokio::spawn(async move {
                        if let Err(e) = fetch_vault_balances_for_pool(
                            &client, &url, &cache, pool_address, vault_a, vault_b,
                        ).await {
                            debug!("Vault fetch failed for {}: {}",
                                pool_address, crate::config::redact_url(&e.to_string()));
                        }
                    });
                }

                // For Meteora DLMM: fetch bitmap extension existence on first discovery
                if matches!(self.state_cache.get_any(&pool_address).map(|p| p.dex_type),
                            Some(crate::router::pool::DexType::MeteoraDlmm))
                {
                    if let Some(pool) = self.state_cache.get_any(&pool_address) {
                        if pool.extra.bitmap_extension.is_none() {
                            let client = self.http_client.clone();
                            let url = self.config.rpc_url.clone();
                            let cache = self.state_cache.clone();
                            let dlmm_program = Pubkey::from_str("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo").unwrap();
                            tokio::spawn(async move {
                                let (bitmap_pda, _) = Pubkey::find_program_address(
                                    &[b"bitmap", pool_address.as_ref()], &dlmm_program,
                                );
                                // Check if it exists on-chain
                                match check_account_exists(&client, &url, &bitmap_pda).await {
                                    Ok(true) => {
                                        if let Some(mut p) = cache.get_any(&pool_address) {
                                            p.extra.bitmap_extension = Some(bitmap_pda);
                                            cache.upsert(pool_address, p);
                                            debug!("DLMM bitmap extension found for {}", pool_address);
                                        }
                                    }
                                    Ok(false) => {
                                        debug!("DLMM bitmap extension not initialized for {}", pool_address);
                                    }
                                    Err(e) => {
                                        debug!("DLMM bitmap check failed: {}", crate::config::redact_url(&e.to_string()));
                                    }
                                }
                            });
                        }
                    }
                }

                // Notify router
                let event = PoolStateChange { pool_address, slot };
                if let Err(e) = tx_sender.try_send(event) {
                    debug!("Channel full, dropping pool change: {}", e);
                }
            }
            _ => {}
        }
    }
}

/// Stats for monitoring Geyser stream health.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct StreamStats {
    pub account_updates_received: u64,
    pub vault_changes_detected: u64,
    pub channel_full_drops: u64,
    pub reconnects: u64,
}

// ─── Per-DEX pool state parsers ───────────────────────────────────────────────

use crate::router::pool::{DexType, PoolExtra, PoolState};

/// Check if an account exists on-chain (not owned by System Program).
async fn check_account_exists(
    client: &reqwest::Client,
    rpc_url: &str,
    account: &Pubkey,
) -> anyhow::Result<bool> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "getAccountInfo",
        "params": [account.to_string(), {"encoding": "base64", "dataSlice": {"offset": 0, "length": 0}}]
    });

    let resp = client
        .post(rpc_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let exists = resp
        .get("result")
        .and_then(|r| r.get("value"))
        .map(|v| !v.is_null())
        .unwrap_or(false);

    Ok(exists)
}

/// Fetch a mint's token program (SPL Token or Token-2022) via getAccountInfo.
/// Returns the owner of the mint account. Caches result in StateCache.
async fn fetch_mint_program(
    client: &reqwest::Client,
    rpc_url: &str,
    cache: &crate::state::StateCache,
    mint: &Pubkey,
) -> anyhow::Result<Pubkey> {
    // Check cache first
    if let Some(prog) = cache.get_mint_program(mint) {
        return Ok(prog);
    }

    let payload = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "getAccountInfo",
        "params": [mint.to_string(), {"encoding": "base64", "dataSlice": {"offset": 0, "length": 0}}]
    });

    let resp = client
        .post(rpc_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let owner_str = resp
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.get("owner"))
        .and_then(|o| o.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing owner in getAccountInfo response"))?;

    let owner = Pubkey::from_str(owner_str)?;
    cache.set_mint_program(*mint, owner);
    debug!("Cached mint program: {} → {}", mint, owner);
    Ok(owner)
}

/// Fetch vault balances for a Raydium AMM/CP pool and update reserves in cache.
/// Uses dataSlice to fetch only the 8-byte balance from each vault.
async fn fetch_vault_balances_for_pool(
    client: &reqwest::Client,
    rpc_url: &str,
    cache: &crate::state::StateCache,
    pool_address: Pubkey,
    vault_a: Pubkey,
    vault_b: Pubkey,
) -> anyhow::Result<()> {
    use base64::{engine::general_purpose, Engine as _};

    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getMultipleAccounts",
        "params": [
            [vault_a.to_string(), vault_b.to_string()],
            { "encoding": "base64", "dataSlice": { "offset": 64, "length": 8 } }
        ]
    });

    let resp = client
        .post(rpc_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let values = resp
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("Invalid getMultipleAccounts response"))?;

    let mut balances = [0u64; 2];
    for (i, value) in values.iter().enumerate().take(2) {
        if value.is_null() { continue; }
        if let Some(b64) = value.get("data").and_then(|d| d.as_array()).and_then(|a| a.first()).and_then(|v| v.as_str()) {
            if let Ok(data) = general_purpose::STANDARD.decode(b64) {
                if data.len() >= 8 {
                    balances[i] = u64::from_le_bytes(data[0..8].try_into().unwrap_or_default());
                }
            }
        }
    }

    // Update pool reserves in cache
    if let Some(mut pool) = cache.get_any(&pool_address) {
        pool.token_a_reserve = balances[0];
        pool.token_b_reserve = balances[1];
        cache.upsert(pool_address, pool);
    }

    Ok(())
}

/// Approximate token reserves from a CLMM sqrt_price_x64 + liquidity.
///
/// reserve_a ≈ L / (sqrt_price / 2^64)  = L * 2^64 / sqrt_price
/// reserve_b ≈ L * sqrt_price / 2^64
fn approx_reserves_from_sqrt_price(sqrt_price_x64: u128, liquidity: u128) -> (u64, u64) {
    if sqrt_price_x64 == 0 || liquidity == 0 {
        return (0, 0);
    }
    let q64: u128 = 1u128 << 64;
    let reserve_a = liquidity
        .checked_mul(q64)
        .and_then(|v| v.checked_div(sqrt_price_x64))
        .unwrap_or(0);
    let reserve_b = liquidity
        .checked_mul(sqrt_price_x64)
        .and_then(|v| v.checked_div(q64))
        .unwrap_or(0);
    let ra = if reserve_a > u64::MAX as u128 { u64::MAX } else { reserve_a as u64 };
    let rb = if reserve_b > u64::MAX as u128 { u64::MAX } else { reserve_b as u64 };
    (ra, rb)
}

// ─── Category A: reserves embedded in pool account ────────────────────────────

/// Parse an Orca Whirlpool pool account (653 bytes).
///
/// Layout (byte offsets):
///   8   discriminator
///   8+1 whirlpools_config (32)
///   49  liquidity (u128, 16 bytes)
///   65  sqrt_price_x64 (u128, 16 bytes)
///   81  tick_current_index (i32, 4 bytes)
///   85  fee_rate (u16), protocol_fee_rate (u16) → 4 bytes
///   89  token_a_protocol_fee (u64) + token_b_protocol_fee (u64) → 16 bytes skip
///   (=105 token_a fees end; however mint_a lands at 101 in practice — use spec offsets)
///   101 token_mint_a (Pubkey, 32 bytes)
///   133 token_vault_a (Pubkey, 32 bytes)
///   181 token_mint_b (Pubkey, 32 bytes)
///   213 token_vault_b (Pubkey, 32 bytes)
pub fn parse_orca_whirlpool(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<PoolState> {
    const MIN_LEN: usize = 245;
    if data.len() < MIN_LEN {
        return None;
    }

    let tick_spacing = u16::from_le_bytes(data[41..43].try_into().ok()?);
    let liquidity = u128::from_le_bytes(data[49..65].try_into().ok()?);
    let sqrt_price_x64 = u128::from_le_bytes(data[65..81].try_into().ok()?);
    let tick = i32::from_le_bytes(data[81..85].try_into().ok()?);
    let mint_a = Pubkey::new_from_array(data[101..133].try_into().ok()?);
    let mint_b = Pubkey::new_from_array(data[181..213].try_into().ok()?);

    let (reserve_a, reserve_b) = approx_reserves_from_sqrt_price(sqrt_price_x64, liquidity);

    Some(PoolState {
        address: *pool_address,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: mint_a,
        token_b_mint: mint_b,
        token_a_reserve: reserve_a,
        token_b_reserve: reserve_b,
        fee_bps: DexType::OrcaWhirlpool.base_fee_bps(),
        current_tick: Some(tick),
        sqrt_price_x64: Some(sqrt_price_x64),
        liquidity: Some(liquidity),
        last_slot: slot,
        extra: {
            let spl_token = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
            PoolExtra {
                vault_a: Some(Pubkey::new_from_array(data[133..165].try_into().ok()?)),
                vault_b: Some(Pubkey::new_from_array(data[213..245].try_into().ok()?)),
                tick_spacing: Some(tick_spacing),
                token_program_a: Some(spl_token),
                token_program_b: Some(spl_token),
                ..Default::default()
            }
        },
        best_bid_price: None,
        best_ask_price: None,
    })
}

/// Parse a Raydium CLMM pool account (1560 bytes).
///
/// Layout (byte offsets):
///   8   discriminator
///   8   bump + padding → 16 total before amm_config
///   16  amm_config (32) → ends at 48
///   48  owner (32) → ends at 80 ← but spec says mint_0 at 73 → use spec
///   73  token_mint_0 (Pubkey, 32)
///   105 token_mint_1 (Pubkey, 32)
///   137 token_vault_0 (Pubkey, 32)
///   169 token_vault_1 (Pubkey, 32)
///   237 liquidity (u128, 16)
///   253 sqrt_price_x64 (u128, 16)
///   269 tick_current (i32, 4)
pub fn parse_raydium_clmm(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<PoolState> {
    const MIN_LEN: usize = 273;
    if data.len() < MIN_LEN {
        return None;
    }

    let amm_config = Pubkey::try_from(&data[9..41]).ok()?;
    let mint_0 = Pubkey::new_from_array(data[73..105].try_into().ok()?);
    let mint_1 = Pubkey::new_from_array(data[105..137].try_into().ok()?);
    let observation_key = Pubkey::try_from(&data[201..233]).ok()?;
    let tick_spacing = u16::from_le_bytes(data[235..237].try_into().ok()?);
    let liquidity = u128::from_le_bytes(data[237..253].try_into().ok()?);
    let sqrt_price_x64 = u128::from_le_bytes(data[253..269].try_into().ok()?);
    let tick = i32::from_le_bytes(data[269..273].try_into().ok()?);

    let (reserve_a, reserve_b) = approx_reserves_from_sqrt_price(sqrt_price_x64, liquidity);

    Some(PoolState {
        address: *pool_address,
        dex_type: DexType::RaydiumClmm,
        token_a_mint: mint_0,
        token_b_mint: mint_1,
        token_a_reserve: reserve_a,
        token_b_reserve: reserve_b,
        fee_bps: DexType::RaydiumClmm.base_fee_bps(),
        current_tick: Some(tick),
        sqrt_price_x64: Some(sqrt_price_x64),
        liquidity: Some(liquidity),
        last_slot: slot,
        extra: {
            let spl_token = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
            PoolExtra {
                vault_a: Some(Pubkey::new_from_array(data[137..169].try_into().ok()?)),
                vault_b: Some(Pubkey::new_from_array(data[169..201].try_into().ok()?)),
                config: Some(amm_config),
                observation: Some(observation_key),
                tick_spacing: Some(tick_spacing),
                token_program_a: Some(spl_token),
                token_program_b: Some(spl_token),
                ..Default::default()
            }
        },
        best_bid_price: None,
        best_ask_price: None,
    })
}

/// Parse a Meteora DLMM pool account (904 bytes).
///
/// Layout (byte offsets):
///   76  active_id (i32, 4)
///   80  bin_step (u16, 2)
///   88  token_x_mint (Pubkey, 32)
///   120 token_y_mint (Pubkey, 32)
///   152 reserve_x (Pubkey vault, 32)
///   184 reserve_y (Pubkey vault, 32)
///
/// Price = (1 + bin_step/10000)^active_id. Synthetic reserves derived from
/// this price and an assumed unit liquidity for route discovery.
pub fn parse_meteora_dlmm(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<PoolState> {
    const MIN_LEN: usize = 216;
    if data.len() < MIN_LEN {
        return None;
    }

    let active_id = i32::from_le_bytes(data[76..80].try_into().ok()?);
    let bin_step = u16::from_le_bytes(data[80..82].try_into().ok()?);
    let mint_x = Pubkey::new_from_array(data[88..120].try_into().ok()?);
    let mint_y = Pubkey::new_from_array(data[120..152].try_into().ok()?);

    // Synthetic reserves: price = (1 + bin_step/10000)^active_id
    // Use integer approximation suitable for route discovery (not simulation).
    // We represent price as a ratio with a fixed denominator of 1_000_000.
    let bin_step_f = bin_step as f64 / 10_000.0;
    let price = (1.0 + bin_step_f).powi(active_id);
    let synthetic_reserve_a: u64 = 1_000_000_000; // 1 token reference amount
    let synthetic_reserve_b: u64 = ((synthetic_reserve_a as f64) * price) as u64;

    Some(PoolState {
        address: *pool_address,
        dex_type: DexType::MeteoraDlmm,
        token_a_mint: mint_x,
        token_b_mint: mint_y,
        token_a_reserve: synthetic_reserve_a,
        token_b_reserve: synthetic_reserve_b,
        fee_bps: DexType::MeteoraDlmm.base_fee_bps(),
        current_tick: Some(active_id),
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: slot,
        extra: {
            let spl_token = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
            let token_2022 = Pubkey::from_str("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb").unwrap();
            // token_mint_x_program_flag at offset 878, y at 879 (0=SPL Token, 1=Token-2022)
            let prog_x = if data.len() > 878 && data[878] == 1 { token_2022 } else { spl_token };
            let prog_y = if data.len() > 879 && data[879] == 1 { token_2022 } else { spl_token };
            PoolExtra {
                vault_a: Some(Pubkey::new_from_array(data[152..184].try_into().ok()?)),
                vault_b: Some(Pubkey::new_from_array(data[184..216].try_into().ok()?)),
                token_program_a: Some(prog_x),
                token_program_b: Some(prog_y),
                ..Default::default()
            }
        },
        best_bid_price: None,
        best_ask_price: None,
    })
}

/// Parse a Meteora DAMM v2 pool account (1112 bytes).
///
/// Layout (byte offsets):
///   0   discriminator (8 bytes): [241, 154, 109, 4, 17, 177, 109, 188]
///   168 token_a_mint (Pubkey, 32)
///   200 token_b_mint (Pubkey, 32)
///   232 a_vault (Pubkey, 32)
///   264 b_vault (Pubkey, 32)
///   360 liquidity (u128, 16)  — used for concentrated mode
///   456 sqrt_price (u128, 16) — used for concentrated mode
///   484 collect_fee_mode (u8): 4 = compounding (direct reserves), 0-3 = concentrated
///   680 token_a_amount (u64, 8) — used when collect_fee_mode == 4
///   688 token_b_amount (u64, 8) — used when collect_fee_mode == 4
pub fn parse_meteora_damm_v2(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<PoolState> {
    const MIN_LEN: usize = 696;
    if data.len() < MIN_LEN {
        return None;
    }

    let mint_a = Pubkey::new_from_array(data[168..200].try_into().ok()?);
    let mint_b = Pubkey::new_from_array(data[200..232].try_into().ok()?);
    let collect_fee_mode = data[484];

    let (reserve_a, reserve_b, sqrt_price_x64, liquidity) = if collect_fee_mode == 4 {
        // Compounding mode: direct reserves stored in account
        let ra = u64::from_le_bytes(data[680..688].try_into().ok()?);
        let rb = u64::from_le_bytes(data[688..696].try_into().ok()?);
        (ra, rb, None, None)
    } else {
        // Concentrated mode: derive from sqrt_price + liquidity
        // Both fields require data.len() >= 472
        if data.len() < 472 {
            return None;
        }
        let liq = u128::from_le_bytes(data[360..376].try_into().ok()?);
        let sqrt_p = u128::from_le_bytes(data[456..472].try_into().ok()?);
        let (ra, rb) = approx_reserves_from_sqrt_price(sqrt_p, liq);
        (ra, rb, Some(sqrt_p), Some(liq))
    };

    Some(PoolState {
        address: *pool_address,
        dex_type: DexType::MeteoraDammV2,
        token_a_mint: mint_a,
        token_b_mint: mint_b,
        token_a_reserve: reserve_a,
        token_b_reserve: reserve_b,
        fee_bps: DexType::MeteoraDammV2.base_fee_bps(),
        current_tick: None,
        sqrt_price_x64,
        liquidity,
        last_slot: slot,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_from_array(data[232..264].try_into().ok()?)),
            vault_b: Some(Pubkey::new_from_array(data[264..296].try_into().ok()?)),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    })
}

// ─── Category B: vault addresses must be fetched separately ──────────────────

/// Parse a Raydium AMM v4 pool account (752 bytes).
///
/// Returns (PoolState, (base_vault, quote_vault)). Reserves are set to 0 until
/// the caller fetches the vault SPL Token accounts and populates them.
///
/// Layout (byte offsets):
///   0   status (u64, first 8 bytes encode pool state; 6 = initialized)
///   336 base_vault (Pubkey, 32)
///   368 quote_vault (Pubkey, 32)
///   400 base_mint (Pubkey, 32)
///   432 quote_mint (Pubkey, 32)
pub fn parse_raydium_amm_v4(
    pool_address: &Pubkey,
    data: &[u8],
    slot: u64,
) -> Option<(PoolState, (Pubkey, Pubkey))> {
    const MIN_LEN: usize = 464;
    if data.len() < MIN_LEN {
        return None;
    }

    let base_vault = Pubkey::new_from_array(data[336..368].try_into().ok()?);
    let quote_vault = Pubkey::new_from_array(data[368..400].try_into().ok()?);
    let base_mint = Pubkey::new_from_array(data[400..432].try_into().ok()?);
    let quote_mint = Pubkey::new_from_array(data[432..464].try_into().ok()?);

    let pool = PoolState {
        address: *pool_address,
        dex_type: DexType::RaydiumAmm,
        token_a_mint: base_mint,
        token_b_mint: quote_mint,
        token_a_reserve: 0, // populated after vault fetch
        token_b_reserve: 0,
        fee_bps: DexType::RaydiumAmm.base_fee_bps(),
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: slot,
        extra: {
            let spl_token = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
            PoolExtra {
                vault_a: Some(base_vault),
                vault_b: Some(quote_vault),
                token_program_a: Some(spl_token),
                token_program_b: Some(spl_token),
                ..Default::default()
            }
        },
        best_bid_price: None,
        best_ask_price: None,
    };

    Some((pool, (base_vault, quote_vault)))
}

/// Parse a Raydium CP (constant-product) pool account (637 bytes).
///
/// Returns (PoolState, (vault_0, vault_1)). Reserves are set to 0 until
/// the caller fetches the vault SPL Token accounts.
///
/// Layout (byte offsets):
///   0   discriminator (8 bytes): [247, 237, 227, 245, 215, 195, 222, 70]
///   8   amm_config (Pubkey, 32)
///   72  token_0_vault (Pubkey, 32)
///   104 token_1_vault (Pubkey, 32)
///   168 token_0_mint (Pubkey, 32)
///   200 token_1_mint (Pubkey, 32)
///   232 token_0_program (Pubkey, 32)
///   264 token_1_program (Pubkey, 32)
pub fn parse_raydium_cp(
    pool_address: &Pubkey,
    data: &[u8],
    slot: u64,
) -> Option<(PoolState, (Pubkey, Pubkey))> {
    const MIN_LEN: usize = 296;
    if data.len() < MIN_LEN {
        return None;
    }

    let amm_config = Pubkey::new_from_array(data[8..40].try_into().ok()?);
    let vault_0 = Pubkey::new_from_array(data[72..104].try_into().ok()?);
    let vault_1 = Pubkey::new_from_array(data[104..136].try_into().ok()?);
    let mint_0 = Pubkey::new_from_array(data[168..200].try_into().ok()?);
    let mint_1 = Pubkey::new_from_array(data[200..232].try_into().ok()?);
    let token_0_program = Pubkey::new_from_array(data[232..264].try_into().ok()?);
    let token_1_program = Pubkey::new_from_array(data[264..296].try_into().ok()?);

    let pool = PoolState {
        address: *pool_address,
        dex_type: DexType::RaydiumCp,
        token_a_mint: mint_0,
        token_b_mint: mint_1,
        token_a_reserve: 0, // populated after vault fetch
        token_b_reserve: 0,
        fee_bps: DexType::RaydiumCp.base_fee_bps(),
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: slot,
        extra: PoolExtra {
            vault_a: Some(vault_0),
            vault_b: Some(vault_1),
            config: Some(amm_config),
            token_program_a: Some(token_0_program),
            token_program_b: Some(token_1_program),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    };

    Some((pool, (vault_0, vault_1)))
}

// ─── Orderbook DEX parsers ──────────────────────────────────────────────────

/// Try to parse a variable-size account as an orderbook DEX market.
fn try_parse_orderbook(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<PoolState> {
    if data.len() >= 624 {
        if let Some(pool) = parse_phoenix_market(pool_address, data, slot) {
            return Some(pool);
        }
    }
    if data.len() >= 256 {
        if let Some(pool) = parse_manifest_market(pool_address, data, slot) {
            return Some(pool);
        }
    }
    None
}

/// Walk a Phoenix sokoban RedBlackTree to find the best (minimum/leftmost) order.
/// Returns (Some(price_in_ticks), depth_base_atoms) or (None, 0) if tree is empty.
///
/// Tree layout at `tree_start`:
///   +0:  root (u32, 1-based index; 0 = SENTINEL = empty)
///   +4:  12 bytes padding
///   +16: NodeAllocator header: size (u64), bump_index (u32), free_list_head (u32)
///   +32: nodes array, each node 64 bytes
///
/// Node layout (64 bytes):
///   +0:  left (u32), right (u32), parent (u32), color (u32) — 16 bytes registers
///   +16: price_in_ticks (u64)
///   +24: order_sequence_number (u64)
///   +32: trader_index (u64)
///   +40: num_base_lots (u64)
///   +48: last_valid_slot (u64)
///   +56: last_valid_unix_timestamp (u64)
fn phoenix_tree_best(data: &[u8], tree_start: usize, base_lot_size: u64) -> (Option<u64>, u64) {
    if data.len() < tree_start + 32 {
        return (None, 0);
    }

    let root = u32::from_le_bytes(
        data[tree_start..tree_start + 4].try_into().unwrap_or([0; 4]),
    );
    if root == 0 {
        return (None, 0);
    }

    let nodes_start = tree_start + 32;

    // Follow left children from root to find minimum (leftmost) node
    let mut current = root;
    for _ in 0..1000 {
        // safety limit against corrupt data
        if current == 0 {
            return (None, 0);
        }
        let node_off = nodes_start + (current as usize - 1) * 64;
        if node_off + 64 > data.len() {
            return (None, 0);
        }

        let left =
            u32::from_le_bytes(data[node_off..node_off + 4].try_into().unwrap_or([0; 4]));
        if left == 0 {
            // Found the minimum node
            let price_in_ticks = u64::from_le_bytes(
                data[node_off + 16..node_off + 24]
                    .try_into()
                    .unwrap_or([0; 8]),
            );
            let num_base_lots = u64::from_le_bytes(
                data[node_off + 40..node_off + 48]
                    .try_into()
                    .unwrap_or([0; 8]),
            );
            return (
                Some(price_in_ticks),
                num_base_lots.saturating_mul(base_lot_size),
            );
        }
        current = left;
    }
    (None, 0)
}

/// Parse a Phoenix V1 market account (header >= 624 bytes, variable total size).
///
/// Layout (byte offsets from MarketHeader):
///   16  bids_size (u64) — number of nodes allocated for bids tree
///   24  asks_size (u64) — number of nodes allocated for asks tree
///   48  base_mint (Pubkey, 32)
///   80  base_vault (Pubkey, 32)
///   136 base_lot_size (u64, 8)
///   152 quote_mint (Pubkey, 32)
///   184 quote_vault (Pubkey, 32)
///   240 quote_lot_size (u64, 8)
///   248 tick_size_in_quote_atoms_per_base_unit (u64, 8)
///
/// FIFOMarket starts at offset 624:
///   +280 taker_fee_bps (u64)
///   +304 bids RedBlackTree starts (offset 928 absolute)
///   asks tree starts at 928 + 32 + bids_size * 64
pub fn parse_phoenix_market(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<PoolState> {
    const HEADER_LEN: usize = 624;
    if data.len() < HEADER_LEN {
        return None;
    }

    let base_mint = Pubkey::new_from_array(data[48..80].try_into().ok()?);
    let quote_mint = Pubkey::new_from_array(data[152..184].try_into().ok()?);
    let base_vault = Pubkey::new_from_array(data[80..112].try_into().ok()?);
    let quote_vault = Pubkey::new_from_array(data[184..216].try_into().ok()?);
    let base_lot_size = u64::from_le_bytes(data[136..144].try_into().ok()?);
    let quote_lot_size = u64::from_le_bytes(data[240..248].try_into().ok()?);

    if base_lot_size == 0 || quote_lot_size == 0 {
        return None;
    }
    if base_mint == Pubkey::default() || quote_mint == Pubkey::default() {
        return None;
    }

    // Read tick_size for price conversion: price = price_in_ticks * tick_size
    let tick_size = u64::from_le_bytes(data[248..256].try_into().ok()?);

    // Read taker fee from FIFOMarket header (offset 624 + 280 = 904)
    let fee_bps = if data.len() > 624 + 288 {
        u64::from_le_bytes(data[624 + 280..624 + 288].try_into().ok()?)
    } else {
        DexType::Phoenix.base_fee_bps()
    };

    // Extract top-of-book from bids and asks RedBlackTrees
    let bids_size = u64::from_le_bytes(data[16..24].try_into().ok()?) as usize;
    let asks_size = u64::from_le_bytes(data[24..32].try_into().ok()?) as usize;

    // Bids tree starts at offset 928
    const BIDS_TREE_START: usize = 928;
    let (best_bid_ticks, bid_depth) = phoenix_tree_best(data, BIDS_TREE_START, base_lot_size);

    // Asks tree starts after bids: 928 + 32 (header) + bids_size * 64 (nodes)
    let asks_tree_start = BIDS_TREE_START + 32 + bids_size * 64;
    let (best_ask_ticks, ask_depth) = phoenix_tree_best(data, asks_tree_start, base_lot_size);
    let _ = asks_size; // used implicitly via tree root/nodes

    // Convert price_in_ticks to quote atoms per base unit
    let best_bid_price = best_bid_ticks.map(|ticks| (ticks as u128) * (tick_size as u128));
    let best_ask_price = best_ask_ticks.map(|ticks| (ticks as u128) * (tick_size as u128));

    Some(PoolState {
        address: *pool_address,
        dex_type: DexType::Phoenix,
        token_a_mint: base_mint,
        token_b_mint: quote_mint,
        token_a_reserve: bid_depth,
        token_b_reserve: ask_depth,
        fee_bps,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: slot,
        extra: PoolExtra {
            vault_a: Some(base_vault),
            vault_b: Some(quote_vault),
            ..Default::default()
        },
        best_bid_price,
        best_ask_price,
    })
}

/// Read a Manifest RestingOrder at the given DataIndex (byte offset into dynamic section).
/// Returns (price_d18: Option<u128>, num_base_atoms: u64).
///
/// DataIndex is a byte offset; absolute position = 256 (MarketFixed header) + index.
/// u32::MAX (0xFFFFFFFF) is the sentinel for an empty book side.
///
/// RBNode<RestingOrder> layout (80 bytes per node):
///   +0:  left (u32), right (u32), parent (u32), color+type+pad — 16 bytes
///   +16: price (u128, LE) — QuoteAtomsPerBaseAtom, D18 fixed-point (scaled by 10^18)
///   +32: num_base_atoms (u64)
///   +40: sequence_number (u64)
///   +48: trader_index (u32)
///   ...
fn manifest_read_order(data: &[u8], index: u32) -> (Option<u128>, u64) {
    if index == u32::MAX {
        return (None, 0);
    }
    let abs_offset = 256 + index as usize;
    // Need at least up to +40 (price u128 at +16..+32, num_base_atoms u64 at +32..+40)
    if abs_offset + 40 > data.len() {
        return (None, 0);
    }

    let price = u128::from_le_bytes(
        data[abs_offset + 16..abs_offset + 32]
            .try_into()
            .unwrap_or([0; 16]),
    );
    let num_base_atoms = u64::from_le_bytes(
        data[abs_offset + 32..abs_offset + 40]
            .try_into()
            .unwrap_or([0; 8]),
    );

    if price == 0 {
        return (None, 0);
    }
    (Some(price), num_base_atoms)
}

/// Parse a Manifest market account (fixed header = 256 bytes, variable total size).
///
/// Layout (byte offsets from MarketFixed):
///   16  base_mint (Pubkey, 32)
///   48  quote_mint (Pubkey, 32)
///   80  base_vault (Pubkey, 32)
///   112 quote_vault (Pubkey, 32)
///   160 bids_best_index (u32) — DataIndex (byte offset into dynamic section)
///   168 asks_best_index (u32)
///
/// Prices are D18 fixed-point (scaled by 10^18). The `get_orderbook_output()`
/// method in pool.rs handles the D18 division for Manifest pools.
pub fn parse_manifest_market(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<PoolState> {
    const HEADER_LEN: usize = 256;
    if data.len() < HEADER_LEN {
        return None;
    }

    let base_mint = Pubkey::new_from_array(data[16..48].try_into().ok()?);
    let quote_mint = Pubkey::new_from_array(data[48..80].try_into().ok()?);
    let base_vault = Pubkey::new_from_array(data[80..112].try_into().ok()?);
    let quote_vault = Pubkey::new_from_array(data[112..144].try_into().ok()?);

    if base_mint == Pubkey::default() || quote_mint == Pubkey::default() {
        return None;
    }

    // Extract top-of-book from best bid/ask indices
    let bids_best_idx = u32::from_le_bytes(data[160..164].try_into().ok()?);
    let asks_best_idx = u32::from_le_bytes(data[168..172].try_into().ok()?);

    let (best_bid_price, bid_depth) = manifest_read_order(data, bids_best_idx);
    let (best_ask_price, ask_depth) = manifest_read_order(data, asks_best_idx);

    Some(PoolState {
        address: *pool_address,
        dex_type: DexType::Manifest,
        token_a_mint: base_mint,
        token_b_mint: quote_mint,
        token_a_reserve: bid_depth,
        token_b_reserve: ask_depth,
        fee_bps: 0,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: slot,
        extra: PoolExtra {
            vault_a: Some(base_vault),
            vault_b: Some(quote_vault),
            ..Default::default()
        },
        best_bid_price,
        best_ask_price,
    })
}
