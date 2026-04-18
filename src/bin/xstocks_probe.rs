//! xStocks price gap validation probe.
//!
//! Polls a configured set of xStock/USDC DEX pools on Solana via JSON-RPC
//! `getMultipleAccounts`, derives spot price from CLMM sqrt_price or AMM
//! reserves, and appends a JSONL record per pool per tick to a log file.
//!
//! Purpose: validate the hypothesis that xStock DEX prices drift from
//! Nasdaq reference prices during market-closed windows (evenings, weekends,
//! holidays) and snap back at market open. Run this over a full Friday-close
//! → Monday-open cycle, then cross-reference against Nasdaq open/close
//! prints in a separate analysis step.
//!
//! Halal posture: xStocks wrapper is Backed Finance SPV-backed. Whitelist
//! here includes only common-stock tokens that generally pass standard
//! Shariah screens (AAOIFI / S&P Dow Jones Shariah). Excludes: bank,
//! insurance, alcohol, tobacco, gambling, interest-bearing ETFs/bonds.
//! Always re-verify per-ticker compliance quarterly before production use.

use anyhow::{Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use std::fs::OpenOptions;
use std::io::Write;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::interval;
use tracing::{info, warn};

use solana_mev_bot::mempool::parsers::{
    parse_meteora_damm_v2, parse_meteora_dlmm, parse_orca_whirlpool, parse_raydium_clmm,
};
use solana_mev_bot::router::pool::PoolState;

/// USDC mint on Solana mainnet.
const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
/// USDC has 6 decimals.
const USDC_DECIMALS: u8 = 6;

#[derive(Debug, Deserialize, Clone)]
struct PoolConfig {
    /// e.g. "AAPLx"
    ticker: String,
    /// Pool account pubkey (base58)
    pool: String,
    /// One of: "OrcaWhirlpool" | "RaydiumClmm" | "MeteoraDlmm" | "MeteoraDammV2"
    dex: String,
    /// Decimal places of the xStock token (USDC is always 6).
    token_decimals: u8,
}

#[derive(Debug, Serialize)]
struct PriceLogEntry {
    ts_ms: u64,
    ticker: String,
    pool: String,
    dex: String,
    /// Computed spot price in USDC per whole token (human units).
    /// None if the pool is in an unparseable state (zero reserves, bad data, etc).
    dex_spot_usdc: Option<f64>,
    token_reserve_raw: u64,
    usdc_reserve_raw: u64,
    token_is_a: bool,
    slot: u64,
    /// Set when something goes wrong and we can't compute a price.
    parse_error: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let rpc_url = std::env::var("RPC_URL").context("RPC_URL env var required")?;
    let config_path = std::env::var("XSTOCKS_PROBE_CONFIG")
        .unwrap_or_else(|_| "xstocks_probe.json".to_string());
    let interval_sec: u64 = std::env::var("XSTOCKS_PROBE_INTERVAL_SEC")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let out_path = std::env::var("XSTOCKS_PROBE_OUT")
        .unwrap_or_else(|_| "/tmp/xstocks_probe.jsonl".to_string());

    let cfg_text = std::fs::read_to_string(&config_path)
        .with_context(|| format!("reading probe config from {}", config_path))?;
    let pools: Vec<PoolConfig> =
        serde_json::from_str(&cfg_text).context("parsing probe config JSON")?;

    if pools.is_empty() {
        anyhow::bail!("no pools configured in {}", config_path);
    }

    info!(
        "xStocks probe starting: {} pools, {}s interval, output → {}",
        pools.len(),
        interval_sec,
        out_path
    );
    for p in &pools {
        info!("  {}  pool={}  dex={}  decimals={}", p.ticker, p.pool, p.dex, p.token_decimals);
    }

    let pool_pubkeys: Vec<Pubkey> = pools
        .iter()
        .map(|p| Pubkey::from_str(&p.pool).with_context(|| format!("bad pubkey for {}", p.ticker)))
        .collect::<Result<Vec<_>>>()?;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let out_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&out_path)
        .with_context(|| format!("opening output file {}", out_path))?;
    let out_file = Mutex::new(out_file);

    let mut tick = interval(Duration::from_secs(interval_sec));
    // skip the immediate first tick so we don't double-fire on startup
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);

    let mut poll_count: u64 = 0;
    loop {
        tokio::select! {
            _ = tick.tick() => {
                poll_count += 1;
                match poll_once(&http, &rpc_url, &pools, &pool_pubkeys, &out_file).await {
                    Ok(n) => {
                        if poll_count % 10 == 1 {
                            info!("poll #{} wrote {} records", poll_count, n);
                        }
                    }
                    Err(e) => warn!("poll #{} failed: {}", poll_count, e),
                }
            }
            _ = &mut shutdown => {
                info!("shutdown requested, exiting after {} polls", poll_count);
                break;
            }
        }
    }
    Ok(())
}

async fn poll_once(
    http: &reqwest::Client,
    rpc_url: &str,
    pools: &[PoolConfig],
    pool_pubkeys: &[Pubkey],
    out_file: &Mutex<std::fs::File>,
) -> Result<usize> {
    let addrs: Vec<String> = pool_pubkeys.iter().map(|p| p.to_string()).collect();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getMultipleAccounts",
        "params": [addrs, {"encoding": "base64"}],
    });

    let resp: serde_json::Value = http
        .post(rpc_url)
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    if let Some(err) = resp.get("error") {
        anyhow::bail!("RPC error: {}", err);
    }

    let slot = resp["result"]["context"]["slot"].as_u64().unwrap_or(0);
    let values = resp["result"]["value"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("no value array in response"))?;

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis() as u64;

    let usdc_mint = Pubkey::from_str(USDC_MINT).unwrap();
    let mut written = 0usize;

    for (pool_cfg, acc) in pools.iter().zip(values.iter()) {
        let entry = compute_entry(pool_cfg, acc, slot, now_ms, &usdc_mint);
        let line = serde_json::to_string(&entry)?;
        {
            let mut f = out_file.lock().unwrap();
            writeln!(f, "{}", line)?;
        }
        written += 1;
    }

    Ok(written)
}

fn compute_entry(
    pool_cfg: &PoolConfig,
    acc: &serde_json::Value,
    slot: u64,
    now_ms: u64,
    usdc_mint: &Pubkey,
) -> PriceLogEntry {
    if acc.is_null() {
        return PriceLogEntry {
            ts_ms: now_ms,
            ticker: pool_cfg.ticker.clone(),
            pool: pool_cfg.pool.clone(),
            dex: pool_cfg.dex.clone(),
            dex_spot_usdc: None,
            token_reserve_raw: 0,
            usdc_reserve_raw: 0,
            token_is_a: true,
            slot,
            parse_error: Some("account not found".into()),
        };
    }

    let data_b64 = match acc["data"][0].as_str() {
        Some(s) => s,
        None => {
            return PriceLogEntry {
                ts_ms: now_ms,
                ticker: pool_cfg.ticker.clone(),
                pool: pool_cfg.pool.clone(),
                dex: pool_cfg.dex.clone(),
                dex_spot_usdc: None,
                token_reserve_raw: 0,
                usdc_reserve_raw: 0,
                token_is_a: true,
                slot,
                parse_error: Some("no data string".into()),
            };
        }
    };

    let data = match base64::engine::general_purpose::STANDARD.decode(data_b64) {
        Ok(d) => d,
        Err(e) => {
            return PriceLogEntry {
                ts_ms: now_ms,
                ticker: pool_cfg.ticker.clone(),
                pool: pool_cfg.pool.clone(),
                dex: pool_cfg.dex.clone(),
                dex_spot_usdc: None,
                token_reserve_raw: 0,
                usdc_reserve_raw: 0,
                token_is_a: true,
                slot,
                parse_error: Some(format!("b64 decode: {}", e)),
            };
        }
    };

    let pool_key = Pubkey::from_str(&pool_cfg.pool).unwrap();

    let parsed: Option<PoolState> = match pool_cfg.dex.as_str() {
        "OrcaWhirlpool" => parse_orca_whirlpool(&pool_key, &data, slot),
        "RaydiumClmm" => parse_raydium_clmm(&pool_key, &data, slot),
        "MeteoraDlmm" => parse_meteora_dlmm(&pool_key, &data, slot),
        "MeteoraDammV2" => parse_meteora_damm_v2(&pool_key, &data, slot),
        _ => None,
    };

    let ps = match parsed {
        Some(p) => p,
        None => {
            return PriceLogEntry {
                ts_ms: now_ms,
                ticker: pool_cfg.ticker.clone(),
                pool: pool_cfg.pool.clone(),
                dex: pool_cfg.dex.clone(),
                dex_spot_usdc: None,
                token_reserve_raw: 0,
                usdc_reserve_raw: 0,
                token_is_a: true,
                slot,
                parse_error: Some(format!("parser returned None ({}B)", data.len())),
            };
        }
    };

    let token_is_a = ps.token_b_mint == *usdc_mint;
    let (token_reserve, usdc_reserve) = if token_is_a {
        (ps.token_a_reserve, ps.token_b_reserve)
    } else if ps.token_a_mint == *usdc_mint {
        (ps.token_b_reserve, ps.token_a_reserve)
    } else {
        return PriceLogEntry {
            ts_ms: now_ms,
            ticker: pool_cfg.ticker.clone(),
            pool: pool_cfg.pool.clone(),
            dex: pool_cfg.dex.clone(),
            dex_spot_usdc: None,
            token_reserve_raw: 0,
            usdc_reserve_raw: 0,
            token_is_a: true,
            slot,
            parse_error: Some(format!(
                "pool pair not *x/USDC: a={} b={}",
                ps.token_a_mint, ps.token_b_mint
            )),
        };
    };

    // Prefer sqrt_price-derived price for CLMM pools (more accurate than
    // approx reserves). Fallback to reserve ratio if sqrt_price absent.
    let dex_spot = if let Some(sqrt_price_x64) = ps.sqrt_price_x64 {
        // CLMM price_a_in_b (raw atoms) = (sqrt_price / 2^64)^2.
        // Token may be on side A or B; we always want USDC/token_A_decimals-adjusted.
        let q128 = 2f64.powi(128);
        let raw_price_b_per_a = (sqrt_price_x64 as f64).powi(2) / q128;
        // raw_price_b_per_a = USDC atoms per 1 token atom (if token_is_a).
        // Human price = raw * 10^token_dec / 10^USDC_dec.
        if token_is_a {
            raw_price_b_per_a
                * 10f64.powi(pool_cfg.token_decimals as i32 - USDC_DECIMALS as i32)
        } else {
            if raw_price_b_per_a == 0.0 {
                0.0
            } else {
                (1.0 / raw_price_b_per_a)
                    * 10f64.powi(pool_cfg.token_decimals as i32 - USDC_DECIMALS as i32)
            }
        }
    } else if token_reserve > 0 {
        let token_units =
            token_reserve as f64 / 10f64.powi(pool_cfg.token_decimals as i32);
        let usdc_units = usdc_reserve as f64 / 10f64.powi(USDC_DECIMALS as i32);
        usdc_units / token_units
    } else {
        0.0
    };

    PriceLogEntry {
        ts_ms: now_ms,
        ticker: pool_cfg.ticker.clone(),
        pool: pool_cfg.pool.clone(),
        dex: pool_cfg.dex.clone(),
        dex_spot_usdc: if dex_spot.is_finite() && dex_spot > 0.0 {
            Some(dex_spot)
        } else {
            None
        },
        token_reserve_raw: token_reserve,
        usdc_reserve_raw: usdc_reserve,
        token_is_a,
        slot,
        parse_error: None,
    }
}
