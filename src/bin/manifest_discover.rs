//! Manifest CLOB market discovery.
//!
//! One-shot tool: enumerates all markets on the Manifest program via
//! `getProgramAccounts` (header-only dataSlice to keep the response small),
//! filters for halal-compatible pairs against a hardcoded mint allowlist
//! (stablecoins + SOL + major LSTs), and for matches fetches vault token
//! balances to surface live depth.
//!
//! Why: the manifest_mm binary is dry-run scaffold; before going LIVE it
//! needs a real halal market to quote on. This probe finds candidates.
//!
//! Output:
//!   - stdout: two-section summary (halal matches + needs-review sample)
//!   - `/tmp/manifest_markets.json`: full JSON with all halal matches
//!
//! Usage:
//!   RPC_URL=https://... cargo run --release --bin manifest_discover
//!
//! No writes to chain. Read-only RPC.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use serde::Serialize;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;
use tracing::info;

use solana_mev_bot::addresses::MANIFEST;
use solana_mev_bot::mempool::parsers::parse_manifest_market;

/// Halal mint allowlist. Keyed by base58 mint → (symbol, decimals).
///
/// Conservative first pass: well-known stables + SOL + major LSTs.
/// Expand after scholar review for things like JLP (leveraged perps vault,
/// currently excluded) or BTC wrappers (generally OK but varies by wrapper).
fn halal_allowlist() -> HashMap<Pubkey, (&'static str, u8)> {
    let entries: &[(&str, &str, u8)] = &[
        // Stablecoins
        ("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "USDC", 6),
        ("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", "USDT", 6),
        ("2b1kV6DkPAnxd5ixfnxCpjxmKwqjjaYmCZfHsFu24GXo", "PYUSD", 6),
        // SOL
        ("So11111111111111111111111111111111111111112", "SOL", 9),
        // Liquid staking tokens
        ("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn", "jitoSOL", 9),
        ("mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So", "mSOL", 9),
        ("bSo13r4TkiE4KumL71LsHTPpL2euBYLFx6h9HP3piy1", "bSOL", 9),
        ("jupSoLaHXQiZZTSfEWMTRRgpnyFm8f6sZdosWBjx93v", "JupSOL", 9),
        ("5oVNBeEEQvYi1cX3ir8Dx5n1P7pdxydbGF2X4TxVusJm", "INF", 9),
        ("BonK1YhkXEGLZzwtcvRTip3gAL9nCeQD7ppZBLXhtTs", "bonkSOL", 9),
    ];
    entries
        .iter()
        .filter_map(|(addr, sym, dec)| Pubkey::from_str(addr).ok().map(|pk| (pk, (*sym, *dec))))
        .collect()
}

#[derive(Serialize)]
struct MarketRecord {
    market: String,
    base_mint: String,
    base_symbol: Option<String>,
    quote_mint: String,
    quote_symbol: Option<String>,
    base_vault: String,
    quote_vault: String,
    base_depth_ui: Option<f64>,
    quote_depth_ui: Option<f64>,
    best_bid_ui: Option<f64>,
    best_ask_ui: Option<f64>,
    halal_match: bool,
}

struct MarketHeader {
    market: Pubkey,
    base_mint: Pubkey,
    quote_mint: Pubkey,
    base_vault: Pubkey,
    quote_vault: Pubkey,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();

    let rpc_url = std::env::var("RPC_URL").context("RPC_URL env not set")?;

    info!("Manifest market discovery starting");
    info!("Manifest program: {}", MANIFEST);

    let halal = halal_allowlist();
    info!("Halal allowlist: {} mints", halal.len());

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;

    // Step 1: enumerate all Manifest accounts with 256-byte header dataSlice.
    //
    // dataSlice caps response at 256 bytes per account — fine for header
    // parse. We'll refetch full data only for halal-matching markets.
    info!("Fetching getProgramAccounts (header only)...");
    let headers = fetch_manifest_headers(&http, &rpc_url).await?;
    info!("Got {} Manifest accounts", headers.len());

    // Step 2: classify. Halal match = both mints in allowlist.
    let mut halal_matches: Vec<MarketHeader> = Vec::new();
    let mut non_halal: Vec<MarketHeader> = Vec::new();
    for h in headers {
        let is_halal = halal.contains_key(&h.base_mint) && halal.contains_key(&h.quote_mint);
        if is_halal {
            halal_matches.push(h);
        } else {
            non_halal.push(h);
        }
    }
    info!(
        "Classified: {} halal-compatible, {} non-halal / needs-review",
        halal_matches.len(),
        non_halal.len()
    );

    // Step 3: refetch halal markets in full to get live best bid/ask + vault depth.
    let mut records: Vec<MarketRecord> = Vec::new();
    for h in &halal_matches {
        let rec = enrich_market(&http, &rpc_url, h, &halal).await?;
        records.push(rec);
    }

    // Sort halal records by quote depth descending (USD proxy).
    records.sort_by(|a, b| {
        b.quote_depth_ui
            .unwrap_or(0.0)
            .partial_cmp(&a.quote_depth_ui.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Step 4: print summary to stdout.
    println!();
    println!("=== HALAL-COMPATIBLE MANIFEST MARKETS ({}) ===", records.len());
    println!(
        "{:<44} {:<12} {:<12} {:>14} {:>14} {:>14} {:>14}",
        "market", "base", "quote", "base_depth", "quote_depth", "best_bid", "best_ask"
    );
    for r in &records {
        println!(
            "{:<44} {:<12} {:<12} {:>14} {:>14} {:>14} {:>14}",
            r.market,
            r.base_symbol.as_deref().unwrap_or("?"),
            r.quote_symbol.as_deref().unwrap_or("?"),
            fmt_opt(r.base_depth_ui),
            fmt_opt(r.quote_depth_ui),
            fmt_opt(r.best_bid_ui),
            fmt_opt(r.best_ask_ui),
        );
    }

    // Non-halal preview: top 10 by raw count (no depth fetch — too expensive).
    println!();
    println!(
        "=== NEEDS-REVIEW SAMPLE (first 10 of {}) ===",
        non_halal.len()
    );
    for h in non_halal.iter().take(10) {
        println!(
            "{}  base={}  quote={}",
            h.market, h.base_mint, h.quote_mint
        );
    }

    // Step 5: write JSON output (halal matches only).
    let out_path = "/tmp/manifest_markets.json";
    std::fs::write(out_path, serde_json::to_string_pretty(&records)?)?;
    info!("Wrote {} halal records to {}", records.len(), out_path);

    Ok(())
}

async fn fetch_manifest_headers(
    http: &reqwest::Client,
    rpc_url: &str,
) -> Result<Vec<MarketHeader>> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getProgramAccounts",
        "params": [
            MANIFEST.to_string(),
            {
                "encoding": "base64",
                "dataSlice": {"offset": 0, "length": 256},
                "commitment": "confirmed"
            }
        ]
    });

    let resp: serde_json::Value = http.post(rpc_url).json(&body).send().await?.json().await?;
    if let Some(err) = resp.get("error") {
        return Err(anyhow!("gPA RPC error: {}", err));
    }

    let arr = resp["result"]
        .as_array()
        .ok_or_else(|| anyhow!("gPA: no result array"))?;

    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        let pubkey_str = entry["pubkey"].as_str().unwrap_or("");
        let data_b64 = entry["account"]["data"][0].as_str().unwrap_or("");
        if pubkey_str.is_empty() || data_b64.is_empty() {
            continue;
        }
        let data = match base64::engine::general_purpose::STANDARD.decode(data_b64) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if data.len() < 256 {
            continue;
        }
        let market = match Pubkey::from_str(pubkey_str) {
            Ok(p) => p,
            Err(_) => continue,
        };
        // Mirror the offsets from mempool/parsers/manifest.rs.
        let base_mint = Pubkey::new_from_array(data[16..48].try_into().unwrap());
        let quote_mint = Pubkey::new_from_array(data[48..80].try_into().unwrap());
        let base_vault = Pubkey::new_from_array(data[80..112].try_into().unwrap());
        let quote_vault = Pubkey::new_from_array(data[112..144].try_into().unwrap());
        if base_mint == Pubkey::default() || quote_mint == Pubkey::default() {
            continue;
        }
        out.push(MarketHeader {
            market,
            base_mint,
            quote_mint,
            base_vault,
            quote_vault,
        });
    }
    Ok(out)
}

async fn enrich_market(
    http: &reqwest::Client,
    rpc_url: &str,
    h: &MarketHeader,
    halal: &HashMap<Pubkey, (&'static str, u8)>,
) -> Result<MarketRecord> {
    let (base_sym, base_dec) = halal[&h.base_mint];
    let (quote_sym, quote_dec) = halal[&h.quote_mint];

    // Single getMultipleAccounts for market(full) + both vaults.
    //
    // Vaults use SPL Token layout (amount u64 at offset 64); we fetch full
    // data and slice in-process to keep one RPC round trip.
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getMultipleAccounts",
        "params": [
            [h.market.to_string(), h.base_vault.to_string(), h.quote_vault.to_string()],
            {"encoding": "base64", "commitment": "confirmed"}
        ]
    });
    let resp: serde_json::Value = http.post(rpc_url).json(&body).send().await?.json().await?;
    let values = resp["result"]["value"].as_array();

    let mut base_depth_ui = None;
    let mut quote_depth_ui = None;
    let mut best_bid_ui = None;
    let mut best_ask_ui = None;

    if let Some(vs) = values {
        // Market full data → best bid/ask via existing parser.
        if let Some(mkt_val) = vs.first() {
            if let Some(b64) = mkt_val["data"][0].as_str() {
                if let Ok(data) = base64::engine::general_purpose::STANDARD.decode(b64) {
                    let slot = resp["result"]["context"]["slot"].as_u64().unwrap_or(0);
                    if let Some(pool) = parse_manifest_market(&h.market, &data, slot) {
                        // Manifest prices are D18 fixed-point, quote_atoms per base_atom.
                        // UI price = price_d18 / 1e18 * 10^(base_dec - quote_dec).
                        let scale = 10f64.powi(base_dec as i32 - quote_dec as i32) / 1e18;
                        best_bid_ui = pool.best_bid_price.map(|p| p as f64 * scale);
                        best_ask_ui = pool.best_ask_price.map(|p| p as f64 * scale);
                    }
                }
            }
        }
        // base_vault: amount at offset 64
        if let Some(bv) = vs.get(1) {
            base_depth_ui = vault_amount_ui(bv, base_dec);
        }
        if let Some(qv) = vs.get(2) {
            quote_depth_ui = vault_amount_ui(qv, quote_dec);
        }
    }

    Ok(MarketRecord {
        market: h.market.to_string(),
        base_mint: h.base_mint.to_string(),
        base_symbol: Some(base_sym.to_string()),
        quote_mint: h.quote_mint.to_string(),
        quote_symbol: Some(quote_sym.to_string()),
        base_vault: h.base_vault.to_string(),
        quote_vault: h.quote_vault.to_string(),
        base_depth_ui,
        quote_depth_ui,
        best_bid_ui,
        best_ask_ui,
        halal_match: true,
    })
}

fn vault_amount_ui(val: &serde_json::Value, decimals: u8) -> Option<f64> {
    let b64 = val["data"][0].as_str()?;
    let data = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    if data.len() < 72 {
        return None;
    }
    let amount = u64::from_le_bytes(data[64..72].try_into().ok()?);
    Some(amount as f64 / 10f64.powi(decimals as i32))
}

fn fmt_opt(x: Option<f64>) -> String {
    match x {
        Some(v) if v.abs() >= 1000.0 => format!("{:.2}", v),
        Some(v) if v.abs() >= 1.0 => format!("{:.4}", v),
        Some(v) => format!("{:.8}", v),
        None => "-".to_string(),
    }
}

