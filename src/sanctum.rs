//! Sanctum LST (Liquid Staking Token) virtual pool management.
//!
//! Handles bootstrapping virtual pools, fetching on-chain LST indices,
//! and updating exchange rates from stake pool accounts.

use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use tracing::{debug, info, warn};

use crate::config;
use crate::addresses;
use crate::router::pool::{DexType, PoolExtra, PoolState};
use crate::state::StateCache;

/// Create Sanctum virtual pools for each supported LST.
///
/// Each LST gets a virtual pool modeling the Sanctum Infinity oracle rate.
/// Reserves are synthetic — large values that produce the correct exchange rate
/// under constant-product math with negligible price impact.
///
/// Initial rates are hardcoded approximations. In production, these should be
/// fetched from on-chain stake pool state at startup (total_lamports / pool_token_supply).
/// The Geyser stream will keep them updated as Sanctum reserve ATAs change.
pub fn bootstrap_pools(state_cache: &StateCache) {
    let sol = config::sol_mint();
    const SYNTHETIC_RESERVE_BASE: u64 = 1_000_000_000_000_000; // 1M SOL in lamports

    // Approximate current exchange rates (SOL per LST).
    // These get corrected as soon as the first Geyser update arrives.
    let lst_rates: Vec<(Pubkey, &str, f64)> = config::lst_mints()
        .into_iter()
        .map(|(mint, name)| {
            let rate = match name {
                "jitoSOL" => 1.271,
                "mSOL" => 1.371,
                "bSOL" => 1.286,
                _ => 1.050, // conservative default
            };
            (mint, name, rate)
        })
        .collect();

    for (lst_mint, name, rate) in &lst_rates {
        // Deterministic virtual pool address: PDA([b"sanctum-virtual", lst_mint], system_program)
        let (virtual_pool_addr, _) = Pubkey::find_program_address(
            &[b"sanctum-virtual", lst_mint.as_ref()],
            &solana_sdk::system_program::id(),
        );

        let reserve_a = SYNTHETIC_RESERVE_BASE;
        let reserve_b = (SYNTHETIC_RESERVE_BASE as f64 / rate) as u64;

        let pool = PoolState {
            address: virtual_pool_addr,
            dex_type: DexType::SanctumInfinity,
            token_a_mint: sol,
            token_b_mint: *lst_mint,
            token_a_reserve: reserve_a,
            token_b_reserve: reserve_b,
            fee_bps: 3, // Sanctum typical fee
            current_tick: None,
            sqrt_price_x64: None,
            liquidity: None,
            last_slot: 0,
            extra: PoolExtra::default(),
            best_bid_price: None,
            best_ask_price: None,
        };

        state_cache.upsert(virtual_pool_addr, pool);
        info!("Bootstrapped Sanctum virtual pool for {}: rate={}, addr={}", name, rate, virtual_pool_addr);
    }
}

/// Fetch the Sanctum LstStateList from on-chain and populate mint->index mapping.
/// Each entry is 80 bytes: mint(32) + calculator(32) + flags(16).
pub async fn bootstrap_lst_indices(
    client: &reqwest::Client,
    rpc_url: &str,
    state_cache: &StateCache,
) -> Result<()> {
    use base64::{engine::general_purpose, Engine as _};

    let s_controller = addresses::SANCTUM_S_CONTROLLER;
    let (lst_state_list_pda, _) = Pubkey::find_program_address(&[b"lst-state-list"], &s_controller);

    let payload = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "getAccountInfo",
        "params": [lst_state_list_pda.to_string(), {"encoding": "base64"}]
    });

    let resp = client.post(rpc_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send().await?
        .json::<serde_json::Value>().await?;

    let b64 = resp["result"]["value"]["data"][0]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("LstStateList account not found"))?;

    let data = general_purpose::STANDARD.decode(b64)?;
    info!("LstStateList: {} bytes", data.len());

    // Parse as array of 80-byte entries, skip 16-byte header
    let header_size = 16;
    if data.len() < header_size { return Ok(()); }
    let entry_data = &data[header_size..];
    let entry_size = 80;
    let count = entry_data.len() / entry_size;

    let mut found = 0;

    for i in 0..count {
        let offset = i * entry_size;
        if offset + entry_size > entry_data.len() { break; }

        // Entry layout: mint(32) + calculator(32) + flags(16) = 80 bytes
        // mint at bytes 0..32, calculator at 32..64
        let mint_bytes: [u8; 32] = entry_data[offset..offset + 32]
            .try_into().unwrap_or([0u8; 32]);
        let mint = Pubkey::new_from_array(mint_bytes);
        if mint == Pubkey::default() { continue; }
        state_cache.set_lst_index(mint, i as u32);
        found += 1;
        // Note: sol_value is NOT stored in LstStateList entries.
        // It's computed dynamically by each LST's SOL value calculator program.
        // Real-time Sanctum rates require calling each calculator via CPI or RPC.
    }

    info!("Bootstrapped {} LST indices from LstStateList", found);
    Ok(())
}

/// Fetch real-time LST/SOL rates from on-chain stake pool accounts.
///
/// Jito + Blaze use SPL Stake Pool layout: total_lamports at offset 258, pool_token_supply at 266.
/// Marinade uses its own layout: msol_price (u64) at offset 512, denominator = 2^32.
///
/// Updates the Sanctum virtual pool reserves in state_cache to reflect current rates.
pub async fn fetch_lst_rates(
    client: &reqwest::Client,
    rpc_url: &str,
    state_cache: &StateCache,
) -> Result<()> {
    use base64::{engine::general_purpose, Engine as _};

    let jito_pool = config::jito_stake_pool();
    let blaze_pool = config::blaze_stake_pool();
    let marinade = config::marinade_state();

    // Batch 1: Jito + Blaze via getMultipleAccounts (SPL Stake Pool layout)
    // total_lamports(u64) at offset 258, pool_token_supply(u64) at offset 266 => 16 bytes from 258
    let payload_spl = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "getMultipleAccounts",
        "params": [
            [jito_pool.to_string(), blaze_pool.to_string()],
            { "encoding": "base64", "dataSlice": { "offset": 258, "length": 16 } }
        ]
    });

    // Batch 2: Marinade via getAccountInfo (custom layout)
    // msol_price(u64) at offset 512 => 8 bytes
    let payload_marinade = serde_json::json!({
        "jsonrpc": "2.0", "id": 2,
        "method": "getAccountInfo",
        "params": [
            marinade.to_string(),
            { "encoding": "base64", "dataSlice": { "offset": 512, "length": 8 } }
        ]
    });

    // Send both requests concurrently
    let (resp_spl, resp_marinade) = tokio::try_join!(
        async {
            client.post(rpc_url)
                .json(&payload_spl)
                .timeout(std::time::Duration::from_secs(10))
                .send().await?
                .json::<serde_json::Value>().await
                .map_err(anyhow::Error::from)
        },
        async {
            client.post(rpc_url)
                .json(&payload_marinade)
                .timeout(std::time::Duration::from_secs(10))
                .send().await?
                .json::<serde_json::Value>().await
                .map_err(anyhow::Error::from)
        },
    )?;

    // Parse Jito + Blaze (SPL Stake Pool)
    let spl_values = resp_spl["result"]["value"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid getMultipleAccounts response for stake pools"))?;

    let lst_names = ["jitoSOL", "bSOL"];
    for (i, value) in spl_values.iter().enumerate().take(2) {
        if value.is_null() {
            warn!("Stake pool account {} not found", if i == 0 { "Jito" } else { "Blaze" });
            continue;
        }
        let b64 = value["data"][0].as_str().unwrap_or_default();
        let data = general_purpose::STANDARD.decode(b64)?;
        if data.len() < 16 {
            warn!("Stake pool {} data too short: {} bytes", lst_names[i], data.len());
            continue;
        }
        let total_lamports = u64::from_le_bytes(data[0..8].try_into().unwrap_or_default());
        let supply = u64::from_le_bytes(data[8..16].try_into().unwrap_or_default());
        if supply == 0 {
            warn!("Stake pool {} has zero supply", lst_names[i]);
            continue;
        }
        let rate = total_lamports as f64 / supply as f64;
        if rate < 0.5 || rate > 5.0 {
            warn!("Stake pool {} rate out of range: {:.6}", lst_names[i], rate);
            continue;
        }
        let mint = config::lst_mints().into_iter()
            .find(|(_, n)| *n == lst_names[i])
            .map(|(m, _)| m);
        if let Some(mint) = mint {
            update_virtual_pool(state_cache, &mint, rate);
            info!("LST rate fetched: {} = {:.6} SOL", lst_names[i], rate);
        }
    }

    // Parse Marinade
    let marinade_b64 = resp_marinade["result"]["value"]["data"][0]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Marinade state account not found"))?;
    let marinade_data = general_purpose::STANDARD.decode(marinade_b64)?;
    if marinade_data.len() >= 8 {
        let msol_price = u64::from_le_bytes(marinade_data[0..8].try_into().unwrap_or_default());
        let rate = msol_price as f64 / 4_294_967_296.0; // 2^32 denominator
        if rate > 0.5 && rate < 5.0 {
            let mint = config::lst_mints().into_iter()
                .find(|(_, n)| *n == "mSOL")
                .map(|(m, _)| m);
            if let Some(mint) = mint {
                update_virtual_pool(state_cache, &mint, rate);
                info!("LST rate fetched: mSOL = {:.6} SOL", rate);
            }
        } else {
            warn!("Marinade mSOL rate out of range: {:.6}", rate);
        }
    }

    Ok(())
}

/// Update a Sanctum virtual pool's reserves to reflect a new LST/SOL rate.
/// Derives the virtual pool PDA, reads from cache, updates reserves, and upserts.
pub fn update_virtual_pool(state_cache: &StateCache, lst_mint: &Pubkey, rate: f64) {
    let (virtual_pool_addr, _) = Pubkey::find_program_address(
        &[b"sanctum-virtual", lst_mint.as_ref()],
        &solana_sdk::system_program::id(),
    );
    if let Some(mut pool) = state_cache.get_any(&virtual_pool_addr) {
        let reserve_a: u64 = 1_000_000_000_000_000; // 1M SOL in lamports
        pool.token_a_reserve = reserve_a;
        pool.token_b_reserve = (reserve_a as f64 / rate) as u64;
        state_cache.upsert(virtual_pool_addr, pool);
        debug!("Updated Sanctum virtual pool {} rate={:.6}", virtual_pool_addr, rate);
    }
}
