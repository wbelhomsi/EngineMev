//! CEX-to-CEX spread observation probe.
//!
//! VALIDATION ONLY — no orders are placed. The question this answers:
//! for a list of mid-cap altcoin pairs, do persistent cross-exchange
//! spreads > 7 bps (a conservative taker-fee floor) actually exist, and
//! do they last long enough (~1s+) to be executable via two-leg orders?
//!
//! Mechanism: subscribes to Binance `@bookTicker` and Bybit `orderbook.1`
//! WebSocket streams for the configured pairs, writes every tick to a
//! JSONL log. Post-hoc analysis (in Python or similar) computes:
//!   - time-aligned cross-exchange bid/ask
//!   - gap histogram (fraction of time with >7 bps edge)
//!   - gap persistence distribution (p50 / p95 dwell time)
//!   - book depth at the gap price (max executable size)
//!
//! Run:
//!   RUST_LOG=info PROBE_PAIRS=SOLUSDT,ETHUSDT,ATOMUSDT,NEARUSDT \
//!     CEX_ARB_PROBE_OUT=/tmp/cex_arb_probe.jsonl \
//!     cargo run --release --bin cex_arb_probe
//!
//! Halal posture: read-only. Pair list should be restricted to tokens
//! whose economics don't carry riba/maysir flags (skip lending-protocol
//! tokens like AAVE/COMP/MKR, skip pure-meme sniping pairs, skip perps).

use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const WS_TIMEOUT: Duration = Duration::from_secs(30);

/// One book-ticker tick, the single row written to the JSONL log.
#[derive(Debug, Serialize)]
struct Tick<'a> {
    ts_ms: u64,
    venue: &'a str,
    symbol: String,
    bid: f64,
    bid_qty: f64,
    ask: f64,
    ask_qty: f64,
}

/// Thin wrapper around the output file so both WS tasks can append atomically.
type LogHandle = std::sync::Arc<Mutex<std::fs::File>>;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn write_tick(log: &LogHandle, tick: &Tick) {
    let line = match serde_json::to_string(tick) {
        Ok(s) => s,
        Err(e) => {
            warn!("serialize tick failed: {}", e);
            return;
        }
    };
    if let Ok(mut f) = log.lock() {
        let _ = writeln!(f, "{}", line);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();

    let pairs_env = std::env::var("PROBE_PAIRS")
        .context("PROBE_PAIRS required (comma-separated, e.g. SOLUSDT,ETHUSDT)")?;
    let pairs: Vec<String> = pairs_env
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    if pairs.is_empty() {
        return Err(anyhow!("PROBE_PAIRS parsed to zero pairs"));
    }

    let out_path = std::env::var("CEX_ARB_PROBE_OUT")
        .unwrap_or_else(|_| "/tmp/cex_arb_probe.jsonl".to_string());
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&out_path)
        .with_context(|| format!("open {}", out_path))?;
    let log: LogHandle = std::sync::Arc::new(Mutex::new(file));

    info!(
        "cex_arb_probe starting — {} pairs, output → {}",
        pairs.len(),
        out_path
    );
    for p in &pairs {
        info!("  pair: {}", p);
    }

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let shutdown_ctrlc = shutdown_tx.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        info!("ctrl-c, shutting down");
        let _ = shutdown_ctrlc.send(true);
    });

    let binance_pairs = pairs.clone();
    let bybit_pairs = pairs.clone();
    let bx_log = log.clone();
    let by_log = log.clone();
    let bx_shutdown = shutdown_rx.clone();
    let by_shutdown = shutdown_rx.clone();

    let bx = tokio::spawn(async move {
        run_binance(binance_pairs, bx_log, bx_shutdown).await;
    });
    let by = tokio::spawn(async move {
        run_bybit(bybit_pairs, by_log, by_shutdown).await;
    });

    let _ = bx.await;
    let _ = by.await;
    info!("cex_arb_probe exited cleanly");
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// Binance — combined @bookTicker streams
// Endpoint: wss://stream.binance.com:9443/stream?streams=solusdt@bookTicker/...
// Payload: {"stream":"solusdt@bookTicker","data":{"u":...,"s":"SOLUSDT",
//           "b":"84.60","B":"10.2","a":"84.61","A":"5.1"}}

async fn run_binance(
    pairs: Vec<String>,
    log: LogHandle,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let streams: String = pairs
        .iter()
        .map(|p| format!("{}@bookTicker", p.to_lowercase()))
        .collect::<Vec<_>>()
        .join("/");
    let url = format!(
        "wss://stream.binance.com:9443/stream?streams={}",
        streams
    );

    loop {
        if *shutdown_rx.borrow() {
            return;
        }
        info!("binance: connecting to {} pair streams", pairs.len());
        let conn = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(&url),
        )
        .await;
        let (ws, _) = match conn {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                warn!("binance: connect failed: {}", e);
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
            Err(_) => {
                warn!("binance: connect timeout");
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
        };
        info!("binance: connected");
        let (_write, mut read) = ws.split();

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() { return; }
                }
                msg = tokio::time::timeout(WS_TIMEOUT, read.next()) => {
                    match msg {
                        Ok(Some(Ok(Message::Text(txt)))) => {
                            if let Err(e) = handle_binance_msg(&txt, &log) {
                                warn!("binance: parse failed: {}", e);
                            }
                        }
                        Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                            warn!("binance: stream closed, reconnecting");
                            break;
                        }
                        Ok(Some(Err(e))) => {
                            warn!("binance: ws error: {}", e);
                            break;
                        }
                        Err(_) => {
                            warn!("binance: idle timeout, reconnecting");
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

fn handle_binance_msg(txt: &str, log: &LogHandle) -> Result<()> {
    let v: serde_json::Value = serde_json::from_str(txt)?;
    let data = v.get("data").unwrap_or(&v);
    let symbol = data
        .get("s")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("no symbol"))?
        .to_string();
    let bid: f64 = data
        .get("b")
        .and_then(|x| x.as_str())
        .unwrap_or("0")
        .parse()
        .unwrap_or(0.0);
    let bid_qty: f64 = data
        .get("B")
        .and_then(|x| x.as_str())
        .unwrap_or("0")
        .parse()
        .unwrap_or(0.0);
    let ask: f64 = data
        .get("a")
        .and_then(|x| x.as_str())
        .unwrap_or("0")
        .parse()
        .unwrap_or(0.0);
    let ask_qty: f64 = data
        .get("A")
        .and_then(|x| x.as_str())
        .unwrap_or("0")
        .parse()
        .unwrap_or(0.0);
    if bid == 0.0 || ask == 0.0 {
        return Ok(());
    }
    write_tick(
        log,
        &Tick {
            ts_ms: now_ms(),
            venue: "binance",
            symbol,
            bid,
            bid_qty,
            ask,
            ask_qty,
        },
    );
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// Bybit v5 spot — orderbook.1 stream (L1 snapshot, same semantics as Binance
// @bookTicker).
// Endpoint: wss://stream.bybit.com/v5/public/spot
// Subscribe: {"op":"subscribe","args":["orderbook.1.SOLUSDT", ...]}
// Payload: {"topic":"orderbook.1.SOLUSDT","ts":..., "type":"snapshot",
//           "data":{"s":"SOLUSDT","b":[["84.60","10.2"]],"a":[["84.61","5.1"]], ...}}

async fn run_bybit(
    pairs: Vec<String>,
    log: LogHandle,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let url = "wss://stream.bybit.com/v5/public/spot";
    // Bybit v5 caps one subscribe request at 10 topics; batch if needed.
    let topic_chunks: Vec<Vec<String>> = pairs
        .chunks(10)
        .map(|c| c.iter().map(|p| format!("orderbook.1.{}", p)).collect())
        .collect();
    let subscribe_msgs: Vec<String> = topic_chunks
        .iter()
        .map(|chunk| {
            serde_json::json!({
                "op": "subscribe",
                "args": chunk,
            })
            .to_string()
        })
        .collect();

    loop {
        if *shutdown_rx.borrow() {
            return;
        }
        info!("bybit: connecting, subscribing {} pairs", pairs.len());
        let conn = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(url),
        )
        .await;
        let (ws, _) = match conn {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                warn!("bybit: connect failed: {}", e);
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
            Err(_) => {
                warn!("bybit: connect timeout");
                tokio::time::sleep(RECONNECT_DELAY).await;
                continue;
            }
        };
        info!("bybit: connected");
        let (mut write, mut read) = ws.split();
        let mut send_failed = false;
        for msg in &subscribe_msgs {
            if let Err(e) = futures::SinkExt::send(
                &mut write,
                Message::Text(msg.clone().into()),
            )
            .await
            {
                warn!("bybit: subscribe send failed: {}", e);
                send_failed = true;
                break;
            }
        }
        if send_failed {
            tokio::time::sleep(RECONNECT_DELAY).await;
            continue;
        }

        // Bybit requires ping every ~20s to keep the connection alive.
        let mut ping_interval = tokio::time::interval(Duration::from_secs(20));
        ping_interval.tick().await; // skip immediate first tick

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() { return; }
                }
                _ = ping_interval.tick() => {
                    let ping = serde_json::json!({"op":"ping"}).to_string();
                    if futures::SinkExt::send(&mut write, Message::Text(ping.into())).await.is_err() {
                        warn!("bybit: ping failed, reconnecting");
                        break;
                    }
                }
                msg = tokio::time::timeout(WS_TIMEOUT, read.next()) => {
                    match msg {
                        Ok(Some(Ok(Message::Text(txt)))) => {
                            if let Err(e) = handle_bybit_msg(&txt, &log) {
                                warn!("bybit: parse failed: {}", e);
                            }
                        }
                        Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                            warn!("bybit: stream closed, reconnecting");
                            break;
                        }
                        Ok(Some(Err(e))) => {
                            warn!("bybit: ws error: {}", e);
                            break;
                        }
                        Err(_) => {
                            warn!("bybit: idle timeout, reconnecting");
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

fn handle_bybit_msg(txt: &str, log: &LogHandle) -> Result<()> {
    let v: serde_json::Value = serde_json::from_str(txt)?;
    // Skip heartbeats / subscribe acks.
    if v.get("op").and_then(|x| x.as_str()) == Some("pong") {
        return Ok(());
    }
    if v.get("success").is_some() {
        return Ok(());
    }
    let topic = match v.get("topic").and_then(|x| x.as_str()) {
        Some(t) if t.starts_with("orderbook.1.") => t,
        _ => return Ok(()),
    };
    let data = v.get("data").ok_or_else(|| anyhow!("no data"))?;
    let symbol = data
        .get("s")
        .and_then(|x| x.as_str())
        .unwrap_or_else(|| topic.trim_start_matches("orderbook.1."))
        .to_string();
    // Bybit ships "b": [[price, qty], ...] and "a": [[price, qty], ...].
    // For orderbook.1 the list has exactly one element (L1).
    let (bid, bid_qty) = parse_bybit_level(data.get("b"));
    let (ask, ask_qty) = parse_bybit_level(data.get("a"));
    if bid == 0.0 && ask == 0.0 {
        // Delta updates that don't carry top-of-book — skip.
        return Ok(());
    }
    write_tick(
        log,
        &Tick {
            ts_ms: now_ms(),
            venue: "bybit",
            symbol,
            bid,
            bid_qty,
            ask,
            ask_qty,
        },
    );
    Ok(())
}

fn parse_bybit_level(v: Option<&serde_json::Value>) -> (f64, f64) {
    let arr = match v.and_then(|x| x.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => return (0.0, 0.0),
    };
    let lvl = match arr[0].as_array() {
        Some(l) if l.len() >= 2 => l,
        _ => return (0.0, 0.0),
    };
    let px: f64 = lvl[0].as_str().unwrap_or("0").parse().unwrap_or(0.0);
    let qty: f64 = lvl[1].as_str().unwrap_or("0").parse().unwrap_or(0.0);
    (px, qty)
}
