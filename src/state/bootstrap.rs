use std::str::FromStr;
use std::time::Duration;

use solana_sdk::pubkey::Pubkey;
use tracing::{info, error, debug};

use crate::router::pool::{DexType, PoolState};
use crate::state::StateCache;

/// Parse a Raydium AMM v4 pool account.
/// Returns (PoolState, vault_a_pubkey, vault_b_pubkey) or None if data is invalid.
///
/// Layout (no Anchor discriminator, 752 bytes):
///   offset 336: coin_vault (Pubkey, 32B)
///   offset 368: pc_vault (Pubkey, 32B)
///   offset 400: coin_vault_mint (Pubkey, 32B)
///   offset 432: pc_vault_mint (Pubkey, 32B)
pub fn parse_raydium_amm_pool(
    pool_address: &Pubkey,
    data: &[u8],
) -> Option<(PoolState, Pubkey, Pubkey)> {
    if data.len() < 464 {
        return None;
    }

    let coin_vault = Pubkey::try_from(&data[336..368]).ok()?;
    let pc_vault = Pubkey::try_from(&data[368..400]).ok()?;
    let coin_mint = Pubkey::try_from(&data[400..432]).ok()?;
    let pc_mint = Pubkey::try_from(&data[432..464]).ok()?;

    let pool = PoolState {
        address: *pool_address,
        dex_type: DexType::RaydiumAmm,
        token_a_mint: coin_mint,
        token_b_mint: pc_mint,
        token_a_reserve: 0,
        token_b_reserve: 0,
        fee_bps: DexType::RaydiumAmm.base_fee_bps(),
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 0,
    };

    Some((pool, coin_vault, pc_vault))
}

/// Parse an Orca Whirlpool pool account.
/// Returns (PoolState, vault_a_pubkey, vault_b_pubkey) or None if data is invalid.
///
/// Layout (8-byte Anchor discriminator, 653 bytes total):
///   offset 49: liquidity (u128 LE, 16B)
///   offset 65: sqrt_price (u128 LE, 16B)
///   offset 81: tick_current_index (i32 LE, 4B)
///   offset 101: token_mint_a (Pubkey, 32B)
///   offset 133: token_vault_a (Pubkey, 32B)
///   offset 181: token_mint_b (Pubkey, 32B)
///   offset 213: token_vault_b (Pubkey, 32B)
pub fn parse_orca_whirlpool_pool(
    pool_address: &Pubkey,
    data: &[u8],
) -> Option<(PoolState, Pubkey, Pubkey)> {
    if data.len() < 245 {
        return None;
    }

    let liquidity = u128::from_le_bytes(data[49..65].try_into().ok()?);
    let sqrt_price = u128::from_le_bytes(data[65..81].try_into().ok()?);
    let tick = i32::from_le_bytes(data[81..85].try_into().ok()?);
    let mint_a = Pubkey::try_from(&data[101..133]).ok()?;
    let vault_a = Pubkey::try_from(&data[133..165]).ok()?;
    let mint_b = Pubkey::try_from(&data[181..213]).ok()?;
    let vault_b = Pubkey::try_from(&data[213..245]).ok()?;

    let pool = PoolState {
        address: *pool_address,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: mint_a,
        token_b_mint: mint_b,
        token_a_reserve: 0,
        token_b_reserve: 0,
        fee_bps: DexType::OrcaWhirlpool.base_fee_bps(),
        current_tick: Some(tick),
        sqrt_price_x64: Some(sqrt_price),
        liquidity: Some(liquidity),
        last_slot: 0,
    };

    Some((pool, vault_a, vault_b))
}

/// Parse a Meteora DLMM LbPair account.
/// Returns (PoolState, reserve_x_pubkey, reserve_y_pubkey) or None if data is invalid.
///
/// Layout (8-byte Anchor discriminator, ~920 bytes):
///   offset 76: active_id (i32 LE, 4B)
///   offset 80: bin_step (u16 LE, 2B)
///   offset 88: token_x_mint (Pubkey, 32B)
///   offset 120: token_y_mint (Pubkey, 32B)
///   offset 152: reserve_x (Pubkey, 32B)
///   offset 184: reserve_y (Pubkey, 32B)
pub fn parse_meteora_dlmm_pool(
    pool_address: &Pubkey,
    data: &[u8],
) -> Option<(PoolState, Pubkey, Pubkey)> {
    if data.len() < 216 {
        return None;
    }

    let _active_id = i32::from_le_bytes(data[76..80].try_into().ok()?);
    let _bin_step = u16::from_le_bytes(data[80..82].try_into().ok()?);
    let mint_x = Pubkey::try_from(&data[88..120]).ok()?;
    let mint_y = Pubkey::try_from(&data[120..152]).ok()?;
    let reserve_x = Pubkey::try_from(&data[152..184]).ok()?;
    let reserve_y = Pubkey::try_from(&data[184..216]).ok()?;

    let pool = PoolState {
        address: *pool_address,
        dex_type: DexType::MeteoraDlmm,
        token_a_mint: mint_x,
        token_b_mint: mint_y,
        token_a_reserve: 0,
        token_b_reserve: 0,
        fee_bps: DexType::MeteoraDlmm.base_fee_bps(),
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 0,
    };

    Some((pool, reserve_x, reserve_y))
}

/// Bootstrap pool state from on-chain data via RPC.
pub async fn bootstrap_pools(
    client: &reqwest::Client,
    rpc_url: &str,
    state_cache: &StateCache,
) -> anyhow::Result<BootstrapStats> {
    let mut stats = BootstrapStats::default();
    let mut all_vaults: Vec<(Pubkey, Pubkey, bool)> = Vec::new();

    match fetch_and_parse_raydium(client, rpc_url, state_cache).await {
        Ok(vaults) => {
            stats.raydium_pools = vaults.len() / 2;
            all_vaults.extend(vaults);
        }
        Err(e) => {
            error!("Raydium bootstrap failed: {}", e);
            stats.errors += 1;
        }
    }

    match fetch_and_parse_orca(client, rpc_url, state_cache).await {
        Ok(vaults) => {
            stats.orca_pools = vaults.len() / 2;
            all_vaults.extend(vaults);
        }
        Err(e) => {
            error!("Orca bootstrap failed: {}", e);
            stats.errors += 1;
        }
    }

    match fetch_and_parse_meteora(client, rpc_url, state_cache).await {
        Ok(vaults) => {
            stats.meteora_pools = vaults.len() / 2;
            all_vaults.extend(vaults);
        }
        Err(e) => {
            error!("Meteora bootstrap failed: {}", e);
            stats.errors += 1;
        }
    }

    // Skip vault balance fetch — Geyser will populate reserves as events arrive.
    // With 1.5M+ vaults, fetching balances at startup would take 15K+ RPC calls.
    // Pools start with reserve=0; the router handles this gracefully (skips routes
    // with zero reserves). Active pools get populated within seconds via Geyser.
    stats.vaults_fetched = 0;

    stats.total_pools = stats.raydium_pools + stats.orca_pools + stats.meteora_pools;
    info!(
        "Bootstrap complete: {} pools ({} Raydium, {} Orca, {} Meteora), {} vaults, {} errors",
        stats.total_pools, stats.raydium_pools, stats.orca_pools, stats.meteora_pools,
        stats.vaults_fetched, stats.errors,
    );

    Ok(stats)
}

#[derive(Debug, Default)]
pub struct BootstrapStats {
    pub raydium_pools: usize,
    pub orca_pools: usize,
    pub meteora_pools: usize,
    pub total_pools: usize,
    pub vaults_fetched: usize,
    pub errors: usize,
}

async fn fetch_and_parse_raydium(
    client: &reqwest::Client,
    rpc_url: &str,
    state_cache: &StateCache,
) -> anyhow::Result<Vec<(Pubkey, Pubkey, bool)>> {
    let program_id = crate::config::programs::raydium_amm();

    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getProgramAccounts",
        "params": [
            program_id.to_string(),
            {
                "encoding": "base64",
                "filters": [
                    { "dataSize": 752 },
                    { "memcmp": { "offset": 0, "bytes": "BgAAAAAAAAA=", "encoding": "base64" } }
                ]
            }
        ]
    });

    let accounts = rpc_get_program_accounts(client, rpc_url, &payload).await?;
    let mut vaults = Vec::new();

    for (pubkey, data) in &accounts {
        if let Some((pool, vault_a, vault_b)) = parse_raydium_amm_pool(pubkey, data) {
            state_cache.upsert(*pubkey, pool);
            state_cache.register_vault(vault_a, *pubkey, true);
            state_cache.register_vault(vault_b, *pubkey, false);
            vaults.push((vault_a, *pubkey, true));
            vaults.push((vault_b, *pubkey, false));
        }
    }

    info!("Raydium: parsed {} pools from {} accounts", vaults.len() / 2, accounts.len());
    Ok(vaults)
}

async fn fetch_and_parse_orca(
    client: &reqwest::Client,
    rpc_url: &str,
    state_cache: &StateCache,
) -> anyhow::Result<Vec<(Pubkey, Pubkey, bool)>> {
    let program_id = crate::config::programs::orca_whirlpool();

    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getProgramAccounts",
        "params": [
            program_id.to_string(),
            {
                "encoding": "base64",
                "filters": [
                    { "dataSize": 653 }
                ]
            }
        ]
    });

    let accounts = rpc_get_program_accounts(client, rpc_url, &payload).await?;
    let mut vaults = Vec::new();

    for (pubkey, data) in &accounts {
        if let Some((pool, vault_a, vault_b)) = parse_orca_whirlpool_pool(pubkey, data) {
            state_cache.upsert(*pubkey, pool);
            state_cache.register_vault(vault_a, *pubkey, true);
            state_cache.register_vault(vault_b, *pubkey, false);
            vaults.push((vault_a, *pubkey, true));
            vaults.push((vault_b, *pubkey, false));
        }
    }

    info!("Orca: parsed {} pools from {} accounts", vaults.len() / 2, accounts.len());
    Ok(vaults)
}

async fn fetch_and_parse_meteora(
    client: &reqwest::Client,
    rpc_url: &str,
    state_cache: &StateCache,
) -> anyhow::Result<Vec<(Pubkey, Pubkey, bool)>> {
    let program_id = crate::config::programs::meteora_dlmm();

    // Try current LbPair size first (920 bytes), then legacy size (902 bytes)
    let mut all_accounts = Vec::new();

    for size in [904u64] {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getProgramAccounts",
            "params": [
                program_id.to_string(),
                {
                    "encoding": "base64",
                    "filters": [
                        { "dataSize": size }
                    ]
                }
            ]
        });

        match rpc_get_program_accounts(client, rpc_url, &payload).await {
            Ok(accounts) => {
                info!("Meteora: fetched {} accounts with dataSize={}", accounts.len(), size);
                all_accounts.extend(accounts);
            }
            Err(e) => {
                debug!("Meteora: no accounts with dataSize={}: {}", size, e);
            }
        }
    }

    let mut vaults = Vec::new();

    for (pubkey, data) in &all_accounts {
        if let Some((pool, vault_a, vault_b)) = parse_meteora_dlmm_pool(pubkey, data) {
            state_cache.upsert(*pubkey, pool);
            state_cache.register_vault(vault_a, *pubkey, true);
            state_cache.register_vault(vault_b, *pubkey, false);
            vaults.push((vault_a, *pubkey, true));
            vaults.push((vault_b, *pubkey, false));
        }
    }

    info!("Meteora: parsed {} pools from {} accounts", vaults.len() / 2, all_accounts.len());
    Ok(vaults)
}

async fn rpc_get_program_accounts(
    client: &reqwest::Client,
    rpc_url: &str,
    payload: &serde_json::Value,
) -> anyhow::Result<Vec<(Pubkey, Vec<u8>)>> {
    use base64::{engine::general_purpose, Engine as _};

    let resp = client
        .post(rpc_url)
        .json(payload)
        .timeout(Duration::from_secs(60))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let accounts = resp
        .get("result")
        .and_then(|r| r.as_array())
        .ok_or_else(|| anyhow::anyhow!("Invalid getProgramAccounts response"))?;

    let mut results = Vec::with_capacity(accounts.len());

    for account in accounts {
        let pubkey_str = account
            .get("pubkey")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing pubkey in account"))?;

        let pubkey = Pubkey::from_str(pubkey_str)?;

        let data_b64 = account
            .get("account")
            .and_then(|a| a.get("data"))
            .and_then(|d| d.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing account data"))?;

        let data = general_purpose::STANDARD.decode(data_b64)?;
        results.push((pubkey, data));
    }

    Ok(results)
}

async fn fetch_vault_balances(
    client: &reqwest::Client,
    rpc_url: &str,
    vault_pubkeys: &[Pubkey],
    state_cache: &StateCache,
) -> anyhow::Result<usize> {
    use base64::{engine::general_purpose, Engine as _};

    let mut total_updated = 0;

    for chunk in vault_pubkeys.chunks(100) {
        let keys: Vec<String> = chunk.iter().map(|p| p.to_string()).collect();

        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getMultipleAccounts",
            "params": [
                keys,
                { "encoding": "base64" }
            ]
        });

        let resp = client
            .post(rpc_url)
            .json(&payload)
            .timeout(Duration::from_secs(30))
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let slot = resp
            .get("result")
            .and_then(|r| r.get("context"))
            .and_then(|c| c.get("slot"))
            .and_then(|s| s.as_u64())
            .unwrap_or(0);

        let values = resp
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("Invalid getMultipleAccounts response"))?;

        for (i, value) in values.iter().enumerate() {
            if value.is_null() {
                continue;
            }

            let data_b64 = value
                .get("data")
                .and_then(|d| d.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.as_str());

            if let Some(b64) = data_b64 {
                if let Ok(data) = general_purpose::STANDARD.decode(b64) {
                    if data.len() >= 72 {
                        let balance = u64::from_le_bytes(
                            data[64..72].try_into().unwrap_or_default()
                        );
                        state_cache.update_vault_balance(&chunk[i], balance, slot);
                        total_updated += 1;
                    }
                }
            }
        }

        debug!("Fetched vault balances: batch of {}, {} updated so far", chunk.len(), total_updated);
    }

    Ok(total_updated)
}
