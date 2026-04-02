use anyhow::Result;
use crossbeam_channel::Sender;
use solana_sdk::pubkey::Pubkey;
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
                    _ => None,
                };

                let Some((pool_state, vault_info)) = parsed else {
                    return;
                };

                // Update cache with parsed pool state
                self.state_cache.upsert(pool_address, pool_state);

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
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_from_array(data[133..165].try_into().ok()?)),
            vault_b: Some(Pubkey::new_from_array(data[213..245].try_into().ok()?)),
            tick_spacing: Some(tick_spacing),
            ..Default::default()
        },
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
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_from_array(data[137..169].try_into().ok()?)),
            vault_b: Some(Pubkey::new_from_array(data[169..201].try_into().ok()?)),
            config: Some(amm_config),
            observation: Some(observation_key),
            tick_spacing: Some(tick_spacing),
            ..Default::default()
        },
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
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_from_array(data[152..184].try_into().ok()?)),
            vault_b: Some(Pubkey::new_from_array(data[184..216].try_into().ok()?)),
            ..Default::default()
        },
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
        extra: PoolExtra {
            vault_a: Some(base_vault),
            vault_b: Some(quote_vault),
            ..Default::default()
        },
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
    };

    Some((pool, (vault_0, vault_1)))
}
