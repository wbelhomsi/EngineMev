use anyhow::Result;
use crossbeam_channel::Sender;
use dashmap::DashMap;
use futures::StreamExt;
use helius_laserstream::grpc::{
    subscribe_update::UpdateOneof,
    CommitmentLevel, SubscribeRequest, SubscribeRequestFilterAccounts,
    SubscribeUpdate,
};
use helius_laserstream::{subscribe, LaserstreamConfig, ChannelOptions};
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;
use tracing::{info, warn, debug};

use crate::addresses;
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
///
/// Max concurrent RPC calls to prevent flooding Helius.
const MAX_CONCURRENT_RPC: usize = 10;
/// Minimum interval between vault fetches for the same pool.
const VAULT_FETCH_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(2);

pub struct GeyserStream {
    config: Arc<BotConfig>,
    state_cache: StateCache,
    http_client: reqwest::Client,
    /// Semaphore to cap concurrent RPC calls.
    rpc_semaphore: Arc<tokio::sync::Semaphore>,
    /// Tracks DLMM pools whose bitmap has been checked (found or not).
    /// Once checked, never re-check — bitmap existence doesn't change.
    bitmap_checked: Arc<DashMap<Pubkey, bool>>,
    /// Last vault fetch time per pool — prevents re-fetching within cooldown.
    vault_last_fetch: Arc<DashMap<Pubkey, Instant>>,
    /// Tracks DLMM pools whose bin arrays have been fetched.
    /// Keyed by (pool_address, active_array_idx) to re-fetch when active bin crosses array boundary.
    bin_arrays_checked: Arc<DashMap<Pubkey, i64>>,
    /// Tracks CLMM pools whose tick arrays have been fetched.
    /// Keyed by (pool_address, tick_array_start_index) to re-fetch when tick crosses array boundary.
    tick_arrays_checked: Arc<DashMap<Pubkey, i32>>,
}

impl GeyserStream {
    pub fn new(config: Arc<BotConfig>, state_cache: StateCache, http_client: reqwest::Client) -> Self {
        Self {
            config,
            state_cache,
            http_client,
            rpc_semaphore: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_RPC)),
            bitmap_checked: Arc::new(DashMap::new()),
            vault_last_fetch: Arc::new(DashMap::new()),
            bin_arrays_checked: Arc::new(DashMap::new()),
            tick_arrays_checked: Arc::new(DashMap::new()),
        }
    }

    /// Start streaming pool state changes via LaserStream gRPC.
    ///
    /// LaserStream handles reconnection, TLS, and Zstd compression internally.
    /// On disconnect, it automatically reconnects with slot-based replay to
    /// avoid missing updates.
    pub async fn start(
        &self,
        tx_sender: Sender<PoolStateChange>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Result<()> {
        let programs = self.config.monitored_programs();
        info!(
            "Starting LaserStream Geyser stream, monitoring {} DEX programs",
            programs.len()
        );

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

        // Subscribe to stake pool state accounts for real-time LST rate updates
        let stake_pool_accounts = vec![
            crate::config::jito_stake_pool().to_string(),
            crate::config::blaze_stake_pool().to_string(),
            crate::config::marinade_state().to_string(),
        ];
        accounts_filter.insert(
            "lst_stake_pools".to_string(),
            SubscribeRequestFilterAccounts {
                account: stake_pool_accounts,
                owner: vec![],
                filters: vec![],
                nonempty_txn_signature: None,
            },
        );

        let subscribe_request = SubscribeRequest {
            accounts: accounts_filter,
            commitment: Some(CommitmentLevel::Processed as i32),
            ..Default::default()
        };

        // LaserStream handles TLS, authentication, reconnection, and Zstd compression.
        let config = LaserstreamConfig::new(
            self.config.geyser_grpc_url.clone(),
            self.config.geyser_auth_token.clone(),
        )
        .with_replay(true)
        .with_channel_options(
            ChannelOptions::default()
                .with_zstd_compression()
        );

        let (stream, _handle) = subscribe(config, subscribe_request);
        tokio::pin!(stream);

        info!("LaserStream subscription active at {}, waiting for account updates...",
              crate::config::redact_url(&self.config.geyser_grpc_url));

        // Main event loop — LaserStream handles reconnection internally
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Geyser stream: shutdown signal received");
                        break;
                    }
                }
                msg = stream.next() => {
                    match msg {
                        Some(Ok(update)) => {
                            self.process_update(update, &tx_sender);
                        }
                        Some(Err(e)) => {
                            warn!("LaserStream error (auto-reconnecting): {}",
                                  crate::config::redact_url(&e.to_string()));
                            // LaserStream handles reconnection internally — continue the loop
                        }
                        None => {
                            warn!("LaserStream stream ended");
                            break;
                        }
                    }
                }
            }
        }

        info!("Geyser stream loop exited");
        Ok(())
    }

    /// Process a Geyser account update.
    ///
    /// Identifies DEX by account data size, dispatches to per-DEX parser,
    /// updates the StateCache, and for Raydium AMM/CP pools triggers an async
    /// vault balance fetch (since those pools don't embed reserves).
    fn process_update(
        &self,
        update: SubscribeUpdate,
        tx_sender: &Sender<PoolStateChange>,
    ) {
        let Some(update_oneof) = update.update_oneof else {
            return;
        };

        if let UpdateOneof::Account(account_update) = update_oneof {
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

                // Check for stake pool account updates (LST rate changes)
                let jito_pool = crate::config::jito_stake_pool();
                let blaze_pool = crate::config::blaze_stake_pool();
                let marinade = crate::config::marinade_state();

                if pool_address == jito_pool || pool_address == blaze_pool {
                    // SPL Stake Pool: total_lamports at offset 258, pool_token_supply at 266
                    if data.len() >= 274 {
                        let total_lamports = u64::from_le_bytes(
                            data[258..266].try_into().unwrap_or_default(),
                        );
                        let supply = u64::from_le_bytes(
                            data[266..274].try_into().unwrap_or_default(),
                        );
                        if supply > 0 {
                            let rate = total_lamports as f64 / supply as f64;
                            if rate > 0.5 && rate < 5.0 {
                                let lst_name = if pool_address == jito_pool { "jitoSOL" } else { "bSOL" };
                                let lst_mint = crate::config::lst_mints()
                                    .into_iter()
                                    .find(|(_, n)| *n == lst_name)
                                    .map(|(m, _)| m);
                                if let Some(mint) = lst_mint {
                                    crate::sanctum::update_virtual_pool(&self.state_cache, &mint, rate);
                                    debug!("Geyser LST rate update: {} = {:.6} SOL", lst_name, rate);
                                }
                            }
                        }
                    }
                    return;
                } else if pool_address == marinade {
                    // Marinade: msol_price at offset 512
                    if data.len() >= 520 {
                        let msol_price = u64::from_le_bytes(
                            data[512..520].try_into().unwrap_or_default(),
                        );
                        let rate = msol_price as f64 / 4_294_967_296.0; // 2^32 denominator
                        if rate > 0.5 && rate < 5.0 {
                            let mint = crate::config::lst_mints()
                                .into_iter()
                                .find(|(_, n)| *n == "mSOL")
                                .map(|(m, _)| m);
                            if let Some(mint) = mint {
                                crate::sanctum::update_virtual_pool(&self.state_cache, &mint, rate);
                                debug!("Geyser LST rate update: mSOL = {:.6} SOL", rate);
                            }
                        }
                    }
                    return;
                }

                // Route to per-DEX parser based on account data size
                let parse_start = Instant::now();
                let (parsed, dex_label) = match data.len() {
                    653 => (
                        parse_orca_whirlpool(&pool_address, data, slot).map(|p| (p, None)),
                        "orca",
                    ),
                    1544 | 1560 => (
                        parse_raydium_clmm(&pool_address, data, slot).map(|p| (p, None)),
                        "raydium_clmm",
                    ),
                    904 => (
                        parse_meteora_dlmm(&pool_address, data, slot).map(|p| (p, None)),
                        "meteora_dlmm",
                    ),
                    1112 => (
                        parse_meteora_damm_v2(&pool_address, data, slot).map(|p| (p, None)),
                        "meteora_damm_v2",
                    ),
                    752 => (
                        parse_raydium_amm_v4(&pool_address, data, slot)
                            .map(|(p, vaults)| (p, Some(vaults))),
                        "raydium_amm",
                    ),
                    637 => (
                        parse_raydium_cp(&pool_address, data, slot)
                            .map(|(p, vaults)| (p, Some(vaults))),
                        "raydium_cp",
                    ),
                    _ => {
                        // Variable-size accounts: try orderbook DEX parsers
                        let ob = try_parse_orderbook(&pool_address, data, slot).map(|p| (p, None));
                        let label = match ob {
                            Some((ref ps, _)) => match ps.dex_type {
                                DexType::Phoenix => "phoenix",
                                DexType::Manifest => "manifest",
                                _ => "unknown",
                            },
                            None => "unknown",
                        };
                        (ob, label)
                    }
                };
                let parse_elapsed_us = parse_start.elapsed().as_micros() as u64;

                let Some((pool_state, vault_info)) = parsed else {
                    // Only record parse errors for known DEX data sizes (not random accounts)
                    if dex_label != "unknown" {
                        crate::metrics::counters::inc_geyser_parse_errors(dex_label);
                    }
                    crate::metrics::counters::record_geyser_parse_duration_us(dex_label, parse_elapsed_us);
                    return;
                };

                crate::metrics::counters::inc_geyser_updates(dex_label);
                crate::metrics::counters::record_geyser_parse_duration_us(dex_label, parse_elapsed_us);

                // Update cache with parsed pool state
                let pool_mints = (pool_state.token_a_mint, pool_state.token_b_mint);
                self.state_cache.upsert(pool_address, pool_state);

                // Fetch token program for uncached mints (fire-and-forget).
                // Only notify the router if BOTH mints are already cached.
                // First event for a new mint pair triggers the fetch; second event
                // (which arrives within ~400ms for active pools) will have cached mints.
                let mut mints_ready = true;
                for mint in [pool_mints.0, pool_mints.1] {
                    if self.state_cache.get_mint_program(&mint).is_none() {
                        mints_ready = false;
                        let client = self.http_client.clone();
                        let url = self.config.rpc_url.clone();
                        let cache = self.state_cache.clone();
                        let sem = self.rpc_semaphore.clone();
                        tokio::spawn(async move {
                            let _permit = sem.acquire().await;
                            let _ = fetch_mint_program(&client, &url, &cache, &mint).await;
                        });
                    }
                }

                // For Category B (Raydium AMM/CP): trigger async vault balance fetch
                // Throttled: skip if we fetched this pool within VAULT_FETCH_COOLDOWN
                if let Some((vault_a, vault_b)) = vault_info {
                    let should_fetch = self.vault_last_fetch
                        .get(&pool_address)
                        .map(|t| t.value().elapsed() >= VAULT_FETCH_COOLDOWN)
                        .unwrap_or(true);
                    if should_fetch {
                        crate::metrics::counters::inc_vault_fetches(dex_label);
                        self.vault_last_fetch.insert(pool_address, Instant::now());
                        let client = self.http_client.clone();
                        let url = self.config.rpc_url.clone();
                        let cache = self.state_cache.clone();
                        let sem = self.rpc_semaphore.clone();
                        tokio::spawn(async move {
                            let _permit = sem.acquire().await;
                            if let Err(e) = fetch_vault_balances_for_pool(
                                &client, &url, &cache, pool_address, vault_a, vault_b,
                            ).await {
                                debug!("Vault fetch failed for {}: {}",
                                    pool_address, crate::config::redact_url(&e.to_string()));
                            }
                        });
                    }
                }

                // For Meteora DLMM: fetch bitmap extension existence on first discovery.
                // Cached permanently in bitmap_checked — never re-check the same pool.
                if matches!(self.state_cache.get_any(&pool_address).map(|p| p.dex_type),
                            Some(crate::router::pool::DexType::MeteoraDlmm))
                    && !self.bitmap_checked.contains_key(&pool_address)
                {
                        self.bitmap_checked.insert(pool_address, false); // mark as in-flight
                        let client = self.http_client.clone();
                        let url = self.config.rpc_url.clone();
                        let cache = self.state_cache.clone();
                        let bitmap_checked = self.bitmap_checked.clone();
                        let sem = self.rpc_semaphore.clone();
                        let dlmm_program = addresses::METEORA_DLMM;
                        tokio::spawn(async move {
                            let _permit = sem.acquire().await;
                            let (bitmap_pda, _) = Pubkey::find_program_address(
                                &[b"bitmap", pool_address.as_ref()], &dlmm_program,
                            );
                            match check_account_exists(&client, &url, &bitmap_pda).await {
                                Ok(true) => {
                                    if let Some(mut p) = cache.get_any(&pool_address) {
                                        p.extra.bitmap_extension = Some(bitmap_pda);
                                        cache.upsert(pool_address, p);
                                    }
                                    bitmap_checked.insert(pool_address, true);
                                    debug!("DLMM bitmap extension found for {}", pool_address);
                                }
                                Ok(false) => {
                                    bitmap_checked.insert(pool_address, false);
                                    debug!("DLMM bitmap not initialized for {}", pool_address);
                                }
                                Err(e) => {
                                    // Remove from checked so we retry on next update
                                    bitmap_checked.remove(&pool_address);
                                    debug!("DLMM bitmap check failed: {}", crate::config::redact_url(&e.to_string()));
                                }
                            }
                        });
                }

                // For Meteora DLMM: fetch bin array accounts for bin-by-bin quoting.
                // Re-fetch when active bin crosses into a different array (array_idx changed).
                if let Some(pool) = self.state_cache.get_any(&pool_address) {
                    if pool.dex_type == crate::router::pool::DexType::MeteoraDlmm {
                        if let Some(active_id) = pool.current_tick {
                            let array_idx = if active_id >= 0 {
                                active_id as i64 / 70
                            } else {
                                (active_id as i64 - 69) / 70
                            };
                            let should_fetch = self.bin_arrays_checked
                                .get(&pool_address)
                                .map(|prev_idx| *prev_idx != array_idx)
                                .unwrap_or(true);
                            if should_fetch {
                                crate::metrics::counters::inc_vault_fetches("meteora_dlmm");
                                self.bin_arrays_checked.insert(pool_address, array_idx);
                                let client = self.http_client.clone();
                                let url = self.config.rpc_url.clone();
                                let cache = self.state_cache.clone();
                                let sem = self.rpc_semaphore.clone();
                                let bin_arrays_checked = self.bin_arrays_checked.clone();
                                let pool_addr = pool_address;
                                let bin_step = pool.fee_bps; // not used in fetch, but keep pool info
                                let _ = bin_step; // suppress unused warning
                                tokio::spawn(async move {
                                    let _permit = sem.acquire().await;
                                    match fetch_dlmm_bin_arrays(
                                        &client, &url, &pool_addr, active_id,
                                    ).await {
                                        Some(arrays) if !arrays.is_empty() => {
                                            debug!(
                                                "DLMM bin arrays fetched for {}: {} arrays",
                                                pool_addr, arrays.len()
                                            );
                                            cache.set_bin_arrays(pool_addr, arrays);
                                        }
                                        _ => {
                                            // Remove so we retry on next update
                                            bin_arrays_checked.remove(&pool_addr);
                                            debug!("DLMM bin array fetch returned empty for {}", pool_addr);
                                        }
                                    }
                                });
                            }
                        }
                    }
                }

                // For CLMM pools (Orca/Raydium): fetch tick array accounts for multi-tick quoting.
                // Re-fetch when current tick crosses into a different array.
                if let Some(pool) = self.state_cache.get_any(&pool_address) {
                    if matches!(pool.dex_type, crate::router::pool::DexType::OrcaWhirlpool
                                             | crate::router::pool::DexType::RaydiumClmm)
                    {
                        if let (Some(tick_current), Some(tick_spacing)) =
                            (pool.current_tick, pool.extra.tick_spacing)
                        {
                            if tick_spacing > 0 {
                                let ticks_per_array: i32 = match pool.dex_type {
                                    crate::router::pool::DexType::OrcaWhirlpool => 88,
                                    _ => 60,
                                };
                                let ticks_in_array = ticks_per_array * tick_spacing as i32;
                                let array_start = if tick_current >= 0 {
                                    (tick_current / ticks_in_array) * ticks_in_array
                                } else {
                                    ((tick_current - ticks_in_array + 1) / ticks_in_array) * ticks_in_array
                                };

                                let should_fetch = self.tick_arrays_checked
                                    .get(&pool_address)
                                    .map(|prev_start| *prev_start != array_start)
                                    .unwrap_or(true);

                                if should_fetch {
                                    let tick_dex_label = match pool.dex_type {
                                        crate::router::pool::DexType::OrcaWhirlpool => "orca",
                                        _ => "raydium_clmm",
                                    };
                                    crate::metrics::counters::inc_vault_fetches(tick_dex_label);
                                    self.tick_arrays_checked.insert(pool_address, array_start);
                                    let client = self.http_client.clone();
                                    let url = self.config.rpc_url.clone();
                                    let cache = self.state_cache.clone();
                                    let sem = self.rpc_semaphore.clone();
                                    let tick_arrays_checked = self.tick_arrays_checked.clone();
                                    let pool_addr = pool_address;
                                    let dex_type = pool.dex_type;
                                    tokio::spawn(async move {
                                        let _permit = sem.acquire().await;
                                        match fetch_clmm_tick_arrays(
                                            &client, &url, &pool_addr, tick_current,
                                            tick_spacing, ticks_per_array, dex_type,
                                        ).await {
                                            Some(arrays) if !arrays.is_empty() => {
                                                debug!(
                                                    "CLMM tick arrays fetched for {}: {} arrays, {} initialized ticks",
                                                    pool_addr, arrays.len(),
                                                    arrays.iter().flat_map(|a| a.ticks.iter())
                                                        .filter(|t| t.liquidity_gross > 0).count(),
                                                );
                                                cache.set_tick_arrays(pool_addr, arrays);
                                            }
                                            _ => {
                                                tick_arrays_checked.remove(&pool_addr);
                                                debug!("CLMM tick array fetch returned empty for {}", pool_addr);
                                            }
                                        }
                                    });
                                }
                            }
                        }
                    }
                }

                // Notify router — only if mint programs are cached
                if mints_ready {
                    let event = PoolStateChange { pool_address, slot };
                    if let Err(e) = tx_sender.try_send(event) {
                        debug!("Channel full, dropping pool change: {}", e);
                    }
                }
            }
    }
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


/// Fetch DLMM bin array accounts around the active bin for a pool.
///
/// Bin array PDA: seeds = [pool_address, bin_array_index.to_le_bytes()]
/// Each bin array holds 70 bins. We fetch 3 arrays: the one containing active_id
/// plus one neighbor on each side, giving coverage of 210 bins total.
///
/// On-chain bin layout per bin: amountX (i64, 8B) + amountY (i64, 8B) + price (u128, 16B) = 32B
/// Bin array header: discriminator (8B) + index (i64, 8B) + version (1B) + padding (1B) + lb_pair (32B) = 50B
/// Total per array: 50 + 70 * 32 = 2290 bytes
async fn fetch_dlmm_bin_arrays(
    client: &reqwest::Client,
    rpc_url: &str,
    pool_address: &Pubkey,
    active_id: i32,
) -> Option<Vec<crate::router::pool::DlmmBinArray>> {
    use base64::Engine;
    use crate::router::pool::{DlmmBin, DlmmBinArray, DLMM_MAX_BIN_PER_ARRAY};

    let dlmm_program = crate::addresses::METEORA_DLMM;
    let bins_per_array = DLMM_MAX_BIN_PER_ARRAY as i64;

    // Determine which array index contains active_id (floor division for negatives)
    let active_array_idx = if active_id >= 0 {
        active_id as i64 / bins_per_array
    } else {
        (active_id as i64 - (bins_per_array - 1)) / bins_per_array
    };

    // Fetch 3 arrays: active one plus neighbors
    let indices = [active_array_idx - 1, active_array_idx, active_array_idx + 1];

    // Derive PDAs for all 3
    let mut addresses = Vec::with_capacity(3);
    for &idx in &indices {
        let (pda, _) = Pubkey::find_program_address(
            &[pool_address.as_ref(), &idx.to_le_bytes()],
            &dlmm_program,
        );
        addresses.push(pda);
    }

    // Batch fetch via getMultipleAccounts
    let pubkeys: Vec<String> = addresses.iter().map(|p| p.to_string()).collect();
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getMultipleAccounts",
        "params": [pubkeys, {"encoding": "base64"}],
    });

    let resp: serde_json::Value = client.post(rpc_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(5))
        .send().await.ok()?
        .json().await.ok()?;

    let accounts = resp.get("result")?.get("value")?.as_array()?;

    let mut result = Vec::new();

    // Bin array layout constants
    const HEADER_SIZE: usize = 50; // 8 (disc) + 8 (index) + 1 (version) + 1 (padding) + 32 (lb_pair)
    const BIN_SIZE: usize = 32;    // 8 (amountX i64) + 8 (amountY i64) + 16 (price u128)
    const EXPECTED_DATA_SIZE: usize = HEADER_SIZE + DLMM_MAX_BIN_PER_ARRAY * BIN_SIZE; // 2290

    for (i, account) in accounts.iter().enumerate() {
        if account.is_null() {
            continue;
        }
        let data_arr = account.get("data")?.as_array()?;
        let b64 = data_arr.first()?.as_str()?;
        let data = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;

        if data.len() < EXPECTED_DATA_SIZE {
            continue;
        }

        let array_idx = indices[i];
        let mut bins = Vec::with_capacity(DLMM_MAX_BIN_PER_ARRAY);

        for b in 0..DLMM_MAX_BIN_PER_ARRAY {
            let offset = HEADER_SIZE + b * BIN_SIZE;
            // amountX and amountY are stored as i64 on-chain; negative values mean zero liquidity
            let amount_x_raw = i64::from_le_bytes(
                data[offset..offset + 8].try_into().ok()?,
            );
            let amount_y_raw = i64::from_le_bytes(
                data[offset + 8..offset + 16].try_into().ok()?,
            );
            let price_q64 = u128::from_le_bytes(
                data[offset + 16..offset + 32].try_into().ok()?,
            );
            bins.push(DlmmBin {
                amount_x: amount_x_raw.max(0) as u64,
                amount_y: amount_y_raw.max(0) as u64,
                price_q64,
            });
        }

        result.push(DlmmBinArray {
            index: array_idx,
            bins,
        });
    }

    Some(result)
}

/// Fetch CLMM tick array accounts for multi-tick swap simulation.
///
/// Fetches 5 tick arrays centered on the current tick position (-2..=+2 from current array).
/// Orca uses string-encoded start_tick_index as PDA seed; Raydium uses big-endian i32 bytes.
///
/// Tick array layout varies by DEX — we auto-detect the tick entry size from account length
/// and parse only the fields we need (liquidity_net and liquidity_gross).
async fn fetch_clmm_tick_arrays(
    client: &reqwest::Client,
    rpc_url: &str,
    pool_address: &Pubkey,
    tick_current: i32,
    tick_spacing: u16,
    ticks_per_array: i32,
    dex_type: crate::router::pool::DexType,
) -> Option<Vec<crate::router::pool::ClmmTickArray>> {
    use base64::Engine;
    use crate::router::pool::{ClmmTick, ClmmTickArray};

    let ticks_in_array = ticks_per_array * tick_spacing as i32;
    if ticks_in_array == 0 {
        return None;
    }

    // Floor division: find the start index of the array containing tick_current
    let start_base = if tick_current >= 0 {
        (tick_current / ticks_in_array) * ticks_in_array
    } else {
        ((tick_current - ticks_in_array + 1) / ticks_in_array) * ticks_in_array
    };

    // Fetch 5 arrays: -2, -1, 0, +1, +2 from current position
    let starts: Vec<i32> = (-2..=2).map(|o| start_base + o * ticks_in_array).collect();

    // Derive PDAs
    let program_id = match dex_type {
        crate::router::pool::DexType::OrcaWhirlpool => crate::addresses::ORCA_WHIRLPOOL,
        crate::router::pool::DexType::RaydiumClmm => crate::addresses::RAYDIUM_CLMM,
        _ => return None,
    };

    let mut pda_list = Vec::with_capacity(5);
    for &start in &starts {
        let pda = match dex_type {
            crate::router::pool::DexType::OrcaWhirlpool => {
                Pubkey::find_program_address(
                    &[b"tick_array", pool_address.as_ref(), start.to_string().as_bytes()],
                    &program_id,
                ).0
            }
            crate::router::pool::DexType::RaydiumClmm => {
                Pubkey::find_program_address(
                    &[b"tick_array", pool_address.as_ref(), &start.to_be_bytes()],
                    &program_id,
                ).0
            }
            _ => continue,
        };
        pda_list.push(pda);
    }

    // Batch fetch via getMultipleAccounts
    let pubkeys: Vec<String> = pda_list.iter().map(|p| p.to_string()).collect();
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getMultipleAccounts",
        "params": [pubkeys, {"encoding": "base64"}],
    });

    let resp: serde_json::Value = client.post(rpc_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(5))
        .send().await.ok()?
        .json().await.ok()?;

    let accounts = resp.get("result")?.get("value")?.as_array()?;

    let mut result = Vec::new();
    let num_ticks = ticks_per_array as usize;

    for (i, account) in accounts.iter().enumerate() {
        if account.is_null() {
            continue;
        }
        let data_arr = account.get("data")?.as_array()?;
        let b64 = data_arr.first()?.as_str()?;
        let data = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;

        let start_tick = starts[i];

        // Determine header size and tick entry size empirically.
        // Orca tick array: 8 (discriminator) + 4 (start_tick_index) = 12 header bytes
        //   remaining / 88 ticks gives tick_entry_size
        // Raydium tick array: 8 (discriminator) + 32 (pool_key) + 8 (start_index i32 padded) = 48 header bytes (approx)
        //   remaining / 60 ticks gives tick_entry_size
        //
        // We try known header sizes first, then fall back to auto-detection.
        let header_size = match dex_type {
            crate::router::pool::DexType::OrcaWhirlpool => {
                // Known Orca header: 8 (disc) + 4 (start_tick_index) = 12
                // But some versions may differ; verify by checking divisibility
                let candidate = 12;
                if (data.len() > candidate) && ((data.len() - candidate) % num_ticks == 0) {
                    candidate
                } else {
                    // Try other common sizes
                    (8..=64).find(|&h| data.len() > h && (data.len() - h) % num_ticks == 0)?
                }
            }
            crate::router::pool::DexType::RaydiumClmm => {
                // Try known Raydium header sizes
                let candidate = 48;
                if (data.len() > candidate) && ((data.len() - candidate) % num_ticks == 0) {
                    candidate
                } else {
                    (8..=64).find(|&h| data.len() > h && (data.len() - h) % num_ticks == 0)?
                }
            }
            _ => continue,
        };

        if data.len() <= header_size {
            continue;
        }
        let tick_entry_size = (data.len() - header_size) / num_ticks;
        if tick_entry_size < 33 {
            continue; // Need at least room for liquidity_net (16) + liquidity_gross (16) + 1
        }

        let mut ticks = Vec::with_capacity(num_ticks);
        for t in 0..num_ticks {
            let offset = header_size + t * tick_entry_size;
            if offset + 33 > data.len() {
                break;
            }

            // Parse tick fields. The exact layout differs:
            // Orca: [initialized: bool(1)] [liquidity_net: i128(16)] [liquidity_gross: u128(16)] ...
            // Raydium: [tick: i32(4)] [liquidity_net: i128(16)] [liquidity_gross: u128(16)] ...
            //
            // Strategy: try both layouts. If layout A gives garbage (liquidity_gross unreasonably large),
            // try layout B. We determine layout from the first entry in the first array.
            let (liq_net, liq_gross) = if dex_type == crate::router::pool::DexType::OrcaWhirlpool {
                // Orca: skip 1 byte (initialized bool), then i128 + u128
                if offset + 33 > data.len() { break; }
                let net = i128::from_le_bytes(data[offset + 1..offset + 17].try_into().ok()?);
                let gross = u128::from_le_bytes(data[offset + 17..offset + 33].try_into().ok()?);
                (net, gross)
            } else {
                // Raydium: skip 4 bytes (tick i32), then i128 + u128
                if offset + 36 > data.len() { break; }
                let net = i128::from_le_bytes(data[offset + 4..offset + 20].try_into().ok()?);
                let gross = u128::from_le_bytes(data[offset + 20..offset + 36].try_into().ok()?);
                (net, gross)
            };

            let tick_index = start_tick + (t as i32) * tick_spacing as i32;

            ticks.push(ClmmTick {
                tick_index,
                liquidity_net: liq_net,
                liquidity_gross: liq_gross,
            });
        }

        result.push(ClmmTickArray {
            start_tick_index: start_tick,
            ticks,
        });
    }

    Some(result)
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
        fee_bps: {
            // fee_rate at offset 45, u16, units of 1/1,000,000 (3000 = 0.3% = 30 bps)
            let fee_rate = u16::from_le_bytes(data[45..47].try_into().ok()?) as u64;
            fee_rate / 100 // convert to bps
        },
        current_tick: Some(tick),
        sqrt_price_x64: Some(sqrt_price_x64),
        liquidity: Some(liquidity),
        last_slot: slot,
        extra: {
            let spl_token = addresses::SPL_TOKEN;
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
        fee_bps: 25, // Default 25 bps (0.25%) — most common CLMM fee tier. Actual fee is in amm_config account.
        current_tick: Some(tick),
        sqrt_price_x64: Some(sqrt_price_x64),
        liquidity: Some(liquidity),
        last_slot: slot,
        extra: {
            let spl_token = addresses::SPL_TOKEN;
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
    // Pitfall #17: active_id max is ~443636, values like 8388608 are garbage
    if active_id.unsigned_abs() > 500_000 {
        return None;
    }
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
            let spl_token = addresses::SPL_TOKEN;
            let token_2022 = addresses::TOKEN_2022;
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
    const MIN_LEN: usize = 624; // need to read up to target_orders at offset 592+32
    if data.len() < MIN_LEN {
        return None;
    }

    let nonce = data[8]; // offset 8, u64 but only lowest byte used

    // Extract trade fee from pool state (more accurate than hardcoded 25 bps)
    let trade_fee_num = u64::from_le_bytes(data[144..152].try_into().ok()?);
    let trade_fee_den = u64::from_le_bytes(data[152..160].try_into().ok()?);
    let fee_bps = if trade_fee_den > 0 {
        trade_fee_num * 10000 / trade_fee_den
    } else {
        25 // fallback
    };

    let base_vault = Pubkey::new_from_array(data[336..368].try_into().ok()?);
    let quote_vault = Pubkey::new_from_array(data[368..400].try_into().ok()?);
    let base_mint = Pubkey::new_from_array(data[400..432].try_into().ok()?);
    let quote_mint = Pubkey::new_from_array(data[432..464].try_into().ok()?);
    let open_orders = Pubkey::new_from_array(data[496..528].try_into().ok()?);
    let market_id = Pubkey::new_from_array(data[528..560].try_into().ok()?);
    let market_program = Pubkey::new_from_array(data[560..592].try_into().ok()?);
    let target_orders = Pubkey::new_from_array(data[592..624].try_into().ok()?);

    let pool = PoolState {
        address: *pool_address,
        dex_type: DexType::RaydiumAmm,
        token_a_mint: base_mint,
        token_b_mint: quote_mint,
        token_a_reserve: 0, // populated after vault fetch
        token_b_reserve: 0,
        fee_bps,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: slot,
        extra: {
            let spl_token = addresses::SPL_TOKEN;
            PoolExtra {
                vault_a: Some(base_vault),
                vault_b: Some(quote_vault),
                token_program_a: Some(spl_token),
                token_program_b: Some(spl_token),
                open_orders: Some(open_orders),
                market: Some(market_id),
                market_program: Some(market_program),
                target_orders: Some(target_orders),
                amm_nonce: Some(nonce),
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
    let asks_tree_start = BIDS_TREE_START + 32 + bids_size.checked_mul(64)?;
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
