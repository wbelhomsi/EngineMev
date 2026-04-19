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
/// Helius LaserStream (Yellowstone gRPC) streams account updates directly
/// from validator memory at sub-50ms latency vs 100-300ms for standard
/// WebSocket.
///
/// Flow:
/// 1. Subscribe by DEX program owner (Raydium, Orca, Meteora, …). We
///    receive updates for every account those programs own — i.e. pool
///    state accounts. NEVER subscribe to SPL Token — that streams every
///    token transfer on Solana.
/// 2. Per-DEX parser in this file decodes the pool state and emits
///    `PoolStateChange`. Raydium AMM v4 / CP are a special case: their
///    pool state doesn't hold reserves, so we do a lazy `getMultipleAccounts`
///    against the vaults (dataSlice 64..72) when those pools change.
/// 3. Downstream router detects price dislocation across DEXes.
/// 4. Bundle submitted for next slot via multi-relay fan-out.
///
/// Max concurrent RPC calls to prevent flooding Helius.
const MAX_CONCURRENT_RPC: usize = 10;
/// Minimum interval between vault fetches for the same pool.
const VAULT_FETCH_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(2);

/// How `GeyserStream` builds its account subscription filter.
///
/// - `WideByOwner` (default, main engine): subscribes to all accounts owned by
///   the DEX programs in `BotConfig::monitored_programs()`. Used for lazy pool
///   discovery and LST stake pool updates — produces thousands of events/sec.
/// - `SpecificAccounts(pools)`: subscribes only to the given pool account
///   pubkeys. Used by the cexdex binary which monitors a fixed, small pool set
///   and would otherwise waste bandwidth + RPC on unrelated pools.
#[derive(Debug, Clone, Default)]
pub enum SubscriptionMode {
    #[default]
    WideByOwner,
    SpecificAccounts(Vec<Pubkey>),
}

pub struct GeyserStream {
    config: Arc<BotConfig>,
    state_cache: StateCache,
    http_client: reqwest::Client,
    subscription_mode: SubscriptionMode,
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
    /// If Some, account updates matching these pubkeys (80 bytes, System
    /// Program-owned) are parsed as nonce accounts and update the pool
    /// instead of falling through to DEX parsers.
    nonce_pool: Option<crate::cexdex::NoncePool>,
    /// Expected authority for managed nonces. Updates with a different
    /// authority are logged and skipped (defense-in-depth).
    nonce_authority: Option<solana_sdk::pubkey::Pubkey>,
}

impl GeyserStream {
    pub fn new(config: Arc<BotConfig>, state_cache: StateCache, http_client: reqwest::Client) -> Self {
        Self {
            config,
            state_cache,
            http_client,
            subscription_mode: SubscriptionMode::default(),
            rpc_semaphore: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_RPC)),
            bitmap_checked: Arc::new(DashMap::new()),
            vault_last_fetch: Arc::new(DashMap::new()),
            bin_arrays_checked: Arc::new(DashMap::new()),
            tick_arrays_checked: Arc::new(DashMap::new()),
            nonce_pool: None,
            nonce_authority: None,
        }
    }

    /// Override the subscription mode. Default is `WideByOwner` (main engine).
    pub fn with_subscription_mode(mut self, mode: SubscriptionMode) -> Self {
        self.subscription_mode = mode;
        self
    }

    /// Register a `NoncePool` whose accounts should be short-circuited to the
    /// nonce parser instead of the DEX parsers when Geyser delivers updates.
    /// The authority is used as a sanity check on each parsed nonce.
    pub fn with_nonce_pool(
        mut self,
        pool: crate::cexdex::NoncePool,
        authority: solana_sdk::pubkey::Pubkey,
    ) -> Self {
        self.nonce_pool = Some(pool);
        self.nonce_authority = Some(authority);
        self
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
        // Build subscription: either wide (by DEX program owner) for the main
        // engine, or narrow (specific account pubkeys) for cexdex.
        let mut accounts_filter: HashMap<String, SubscribeRequestFilterAccounts> = HashMap::new();

        match &self.subscription_mode {
            SubscriptionMode::WideByOwner => {
                let programs = self.config.monitored_programs();
                info!(
                    "Starting LaserStream Geyser stream (wide), monitoring {} DEX programs",
                    programs.len()
                );
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

                // Stake pool state accounts for real-time LST rate updates —
                // only relevant when the main engine's LST arb is enabled.
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
            }
            SubscriptionMode::SpecificAccounts(pools) => {
                info!(
                    "Starting LaserStream Geyser stream (narrow), monitoring {} specific pool accounts",
                    pools.len()
                );
                accounts_filter.insert(
                    "narrow_pools".to_string(),
                    SubscribeRequestFilterAccounts {
                        account: pools.iter().map(|p| p.to_string()).collect(),
                        owner: vec![],
                        filters: vec![],
                        nonempty_txn_signature: None,
                    },
                );
            }
        }

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

                // Nonce short-circuit: 80 bytes, System Program owner, registered pubkey.
                // Handled before pool-parser dispatch and returns without forwarding
                // a PoolStateChange event (nonces aren't pool state).
                if data.len() == 80 {
                    if let (Some(np), Some(auth)) = (&self.nonce_pool, &self.nonce_authority) {
                        if account_info.owner == solana_system_interface::program::id().to_bytes().to_vec()
                            && np.contains(&pool_address)
                        {
                            if let Some((parsed_auth, hash)) =
                                crate::mempool::parsers::parse_nonce(data)
                            {
                                if &parsed_auth == auth {
                                    np.update_cached_hash(pool_address, hash);
                                } else {
                                    tracing::warn!(
                                        "Nonce {} authority mismatch: parsed={} expected={}",
                                        pool_address, parsed_auth, auth,
                                    );
                                }
                            }
                            return;
                        }
                    }
                }

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
                        // PumpSwap: 243-301 bytes, discriminator-routed (no overlap with existing DEX sizes)
                        if data.len() >= 243 && data[0..8] == [0xf1, 0x9a, 0x6d, 0x04, 0x11, 0xb1, 0x6d, 0xbc] {
                            let ps = parse_pumpswap(&pool_address, data, slot)
                                .map(|(p, vaults)| (p, Some(vaults)));
                            (ps, "pumpswap")
                        } else {
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
                // DLMM and Raydium CP parsers read actual token programs from pool data.
                // Other parsers (Orca, Raydium CLMM, Raydium AMM) hardcode SPL Token which
                // is wrong for Token-2022 pools — skip caching from those to let the RPC
                // fetch provide the authoritative value.
                let parser_knows_programs = matches!(
                    pool_state.dex_type,
                    crate::router::pool::DexType::MeteoraDlmm
                    | crate::router::pool::DexType::RaydiumCp
                    | crate::router::pool::DexType::RaydiumAmm // AMM v4 is always SPL Token (predates Token-2022)
                );
                if parser_knows_programs {
                    if let Some(prog) = pool_state.extra.token_program_a {
                        if self.state_cache.get_mint_program(&pool_state.token_a_mint).is_none() {
                            self.state_cache.set_mint_program(pool_state.token_a_mint, prog);
                        }
                    }
                    if let Some(prog) = pool_state.extra.token_program_b {
                        if self.state_cache.get_mint_program(&pool_state.token_b_mint).is_none() {
                            self.state_cache.set_mint_program(pool_state.token_b_mint, prog);
                        }
                    }
                }
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

                // Vault fetch strategy:
                // - Raydium AMM/CP: MUST fetch vaults (reserves not in pool state)
                // - Orca/CLMM/DAMM v2/DLMM: MAY fetch vaults once to get token programs
                //   (vault.owner is the token program — SPL Token or Token-2022)
                //   Only fetch if either mint's token program isn't already cached.
                let (vault_a, vault_b) = match vault_info {
                    Some(v) => v,
                    None => {
                        // Try to pull vaults from pool extra (Orca/CLMM/DAMM v2/DLMM)
                        if let Some(p) = self.state_cache.get_any(&pool_address) {
                            let a_known = self.state_cache.get_mint_program(&p.token_a_mint).is_some();
                            let b_known = self.state_cache.get_mint_program(&p.token_b_mint).is_some();
                            if a_known && b_known {
                                (Pubkey::default(), Pubkey::default()) // skip fetch
                            } else if let (Some(va), Some(vb)) = (p.extra.vault_a, p.extra.vault_b) {
                                (va, vb)
                            } else {
                                (Pubkey::default(), Pubkey::default())
                            }
                        } else {
                            (Pubkey::default(), Pubkey::default())
                        }
                    }
                };

                // Throttled: skip if we fetched this pool within VAULT_FETCH_COOLDOWN
                if vault_a != Pubkey::default() && vault_b != Pubkey::default() {
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

// ─── Per-DEX pool state parsers (moved to mempool/parsers/) ───────────────────

use crate::router::pool::DexType;
use crate::mempool::parsers::{
    parse_orca_whirlpool, parse_raydium_clmm, parse_raydium_amm_v4, parse_raydium_cp,
    parse_meteora_dlmm, parse_meteora_damm_v2, try_parse_orderbook, parse_pumpswap,
};

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
    use std::str::FromStr;

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
    let mut vault_owners: [Option<Pubkey>; 2] = [None, None];
    for (i, value) in values.iter().enumerate().take(2) {
        if value.is_null() { continue; }
        // Parse balance from dataSlice (8 bytes at offset 64 of token account = amount)
        if let Some(b64) = value.get("data").and_then(|d| d.as_array()).and_then(|a| a.first()).and_then(|v| v.as_str()) {
            if let Ok(data) = general_purpose::STANDARD.decode(b64) {
                if data.len() >= 8 {
                    balances[i] = u64::from_le_bytes(data[0..8].try_into().unwrap_or_default());
                }
            }
        }
        // The vault account's owner IS the token program (SPL Token or Token-2022)
        if let Some(owner_str) = value.get("owner").and_then(|v| v.as_str()) {
            if let Ok(owner) = Pubkey::from_str(owner_str) {
                vault_owners[i] = Some(owner);
            }
        }
    }

    // Update pool: reserves only for Raydium AMM/CP (other DEXes derive reserves
    // from sqrt_price/bins). Token programs (vault owners) always cached.
    if let Some(mut pool) = cache.get_any(&pool_address) {
        let updates_reserves = matches!(
            pool.dex_type,
            crate::router::pool::DexType::RaydiumAmm | crate::router::pool::DexType::RaydiumCp
        );
        if updates_reserves {
            pool.token_a_reserve = balances[0];
            pool.token_b_reserve = balances[1];
        }
        // Vault owner = token program for that mint. Authoritative source.
        if let Some(owner) = vault_owners[0] {
            cache.set_mint_program(pool.token_a_mint, owner);
            pool.extra.token_program_a = Some(owner);
        }
        if let Some(owner) = vault_owners[1] {
            cache.set_mint_program(pool.token_b_mint, owner);
            pool.extra.token_program_b = Some(owner);
        }
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

