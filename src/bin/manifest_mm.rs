//! Manifest CLOB market-making binary (v1 — DRY RUN SCAFFOLD).
//!
//! This first cut is a dry-run quoting probe: it connects to Binance WS,
//! derives a target (bid, ask) every N ms using the `Quoter`, builds the
//! corresponding `BatchUpdate` instructions, and logs them to JSONL
//! WITHOUT submitting them on-chain. Purpose: validate quoting math
//! against live CEX prices before committing capital or building the
//! full on-chain submission + fill-detection loop.
//!
//! What's LIVE in this v1:
//!   - Binance WS SOL/USDC feed (reused from `src/feed/binance.rs`)
//!   - Quoter → target bid/ask with inventory skew
//!   - BatchUpdate IX construction (cancel + place)
//!   - JSONL quote-decision log
//!
//! What's STUBBED / pending follow-up commits:
//!   - RPC blockhash fetch + tx signing + bundle submission
//!   - Fill detection (Geyser poll of market account)
//!   - Inventory state sync from on-chain seat balance
//!   - Binance hedge execution
//!   - Prometheus metrics
//!
//! Run:
//!   MM_DRY_RUN=true cargo run --release --bin manifest_mm

use anyhow::{Context, Result};
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use tracing::{info, warn};

use solana_mev_bot::cexdex::price_store::PriceStore;
use solana_mev_bot::executor::swaps::manifest_mm::{
    build_batch_update_ix, order_type, CancelOrderParams, PlaceOrderParams,
};
use solana_mev_bot::feed::binance::run_solusdc_loop;
use solana_mev_bot::mm::{BookState, MmConfig, Quoter, QuoterConfig};

#[derive(Debug, Serialize)]
struct QuoteLogEntry {
    ts_ms: u64,
    cex_mid: f64,
    cex_bid: f64,
    cex_ask: f64,
    inventory_ratio: f64,
    target_bid: f64,
    target_ask: f64,
    // Raw on-chain representation of the quoted prices (for IX debugging).
    bid_mantissa: u32,
    bid_exponent: i8,
    ask_mantissa: u32,
    ask_exponent: i8,
    ix_data_bytes: usize,
    // How we'd act — but in dry-run we don't actually submit.
    would_cancel_count: usize,
    would_place_count: usize,
    dry_run: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = MmConfig::from_env().context("loading MmConfig")?;

    if cfg.cex_reference_symbol != "SOLUSDC" {
        // Hardcoded Binance feed is SOL/USDC today; non-SOL markets need
        // additional WS integration before running live.
        warn!(
            "MM_CEX_REFERENCE_SYMBOL={} but only SOLUSDC is wired; quoting will be stubbed",
            cfg.cex_reference_symbol
        );
    }

    info!(
        "manifest_mm starting — market={} base={} quote={} dry_run={} run_secs={}",
        cfg.market, cfg.base_mint, cfg.quote_mint, cfg.dry_run, cfg.run_secs,
    );

    // Ensure stats path parent exists.
    let stats_base = std::path::PathBuf::from(&cfg.stats_path);
    if let Some(parent) = stats_base.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let log_path = format!(
        "{}-{}.quotes.jsonl",
        cfg.stats_path,
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
    );
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .context("opening quote log")?;
    let log_file = Arc::new(Mutex::new(log_file));
    info!("writing quote decisions to {}", log_path);

    // Shared CEX price store.
    let price_store = PriceStore::new();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Binance WS task.
    let binance_store = price_store.clone();
    let binance_shutdown = shutdown_rx.clone();
    let binance_handle =
        tokio::spawn(async move { run_solusdc_loop(binance_store, binance_shutdown).await });

    // Quoter.
    let quoter = Quoter::new(QuoterConfig {
        half_spread_frac: cfg.half_spread_frac,
        max_skew_frac: cfg.max_skew_frac,
        target_inventory_ratio: cfg.target_inventory_ratio,
        skew_ratio_window: cfg.skew_ratio_window,
        min_half_spread_frac: cfg.min_half_spread_frac,
        order_size_base_atoms: cfg.order_size_base_atoms,
    });

    let mut book = BookState::new();

    // Fake payer — in dry-run we don't sign, but we do build the IX to
    // verify shape and size. Live mode will replace this with the real
    // keypair loaded from MM_SEARCHER_PRIVATE_KEY.
    let payer = solana_sdk::pubkey::Pubkey::new_unique();

    // Start time and auto-shutdown tracking.
    let start = Instant::now();
    let run_duration = if cfg.run_secs > 0 {
        Some(Duration::from_secs(cfg.run_secs))
    } else {
        None
    };

    // Ctrl-C handler.
    let shutdown_tx_ctrlc = shutdown_tx.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        info!("ctrl-c received, shutting down");
        let _ = shutdown_tx_ctrlc.send(true);
    });

    let mut tick = tokio::time::interval(Duration::from_millis(cfg.requote_interval_ms));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut quote_count: u64 = 0;

    loop {
        tokio::select! {
            _ = tick.tick() => {
                if let Some(d) = run_duration {
                    if start.elapsed() >= d {
                        info!("run_secs reached, shutting down");
                        let _ = shutdown_tx.send(true);
                        break;
                    }
                }
                quote_count += 1;

                // Pull fresh CEX snapshot.
                if price_store.is_stale(&cfg.cex_reference_symbol, cfg.cex_staleness_ms) {
                    if quote_count % 20 == 1 {
                        warn!("CEX snapshot stale, skipping quote cycle");
                    }
                    continue;
                }
                let Some(cex) = price_store.get_cex(&cfg.cex_reference_symbol) else {
                    continue;
                };

                // TODO: wire real inventory when seat sync is built.
                // For dry run, assume balanced portfolio.
                let inventory_ratio = cfg.target_inventory_ratio;

                let decision = quoter.quote(cex.mid(), inventory_ratio);

                // Encode prices as mantissa/exponent pairs. Manifest uses
                // u32 mantissa + i8 exponent; choose exponent to keep
                // mantissa within u32::MAX while preserving ≥6 sig figs.
                let (bid_m, bid_e) = encode_price_mantissa(decision.bid_price);
                let (ask_m, ask_e) = encode_price_mantissa(decision.ask_price);

                let cancels: Vec<CancelOrderParams> = book
                    .seq_numbers()
                    .into_iter()
                    .map(|seq| CancelOrderParams {
                        order_sequence_number: seq,
                        order_index_hint: None,
                    })
                    .collect();
                let new_orders = vec![
                    PlaceOrderParams {
                        base_atoms: decision.bid_size_base_atoms,
                        price_mantissa: bid_m,
                        price_exponent: bid_e,
                        is_bid: true,
                        last_valid_slot: 0,
                        order_type: order_type::POST_ONLY,
                    },
                    PlaceOrderParams {
                        base_atoms: decision.ask_size_base_atoms,
                        price_mantissa: ask_m,
                        price_exponent: ask_e,
                        is_bid: false,
                        last_valid_slot: 0,
                        order_type: order_type::POST_ONLY,
                    },
                ];

                let ix = build_batch_update_ix(
                    &payer,
                    &cfg.market,
                    cancels.clone(),
                    new_orders.clone(),
                );

                let entry = QuoteLogEntry {
                    ts_ms: SystemTime::now()
                        .duration_since(UNIX_EPOCH)?
                        .as_millis() as u64,
                    cex_mid: cex.mid(),
                    cex_bid: cex.best_bid_usd,
                    cex_ask: cex.best_ask_usd,
                    inventory_ratio,
                    target_bid: decision.bid_price,
                    target_ask: decision.ask_price,
                    bid_mantissa: bid_m,
                    bid_exponent: bid_e,
                    ask_mantissa: ask_m,
                    ask_exponent: ask_e,
                    ix_data_bytes: ix.data.len(),
                    would_cancel_count: cancels.len(),
                    would_place_count: new_orders.len(),
                    dry_run: cfg.dry_run,
                };
                {
                    let line = serde_json::to_string(&entry)?;
                    let mut f = log_file.lock().unwrap();
                    writeln!(f, "{}", line)?;
                }

                if quote_count % 20 == 1 {
                    info!(
                        "quote #{}: mid={:.4} bid={:.4} ask={:.4} ix={}B",
                        quote_count,
                        cex.mid(),
                        decision.bid_price,
                        decision.ask_price,
                        ix.data.len(),
                    );
                }

                if cfg.dry_run {
                    // Emulate success: treat the "placed" orders as live with
                    // synthetic sequence numbers (high u64 watermark).
                    // This lets the BookState track cancel-counts correctly.
                    book.clear();
                    let synth_seq_base = quote_count * 2;
                    book.insert(solana_mev_bot::mm::LiveOrder {
                        seq_number: synth_seq_base,
                        side: solana_mev_bot::mm::OrderSide::Bid,
                        price_mantissa: bid_m,
                        price_exponent: bid_e,
                        base_atoms: decision.bid_size_base_atoms,
                        placed_at: Instant::now(),
                    });
                    book.insert(solana_mev_bot::mm::LiveOrder {
                        seq_number: synth_seq_base + 1,
                        side: solana_mev_bot::mm::OrderSide::Ask,
                        price_mantissa: ask_m,
                        price_exponent: ask_e,
                        base_atoms: decision.ask_size_base_atoms,
                        placed_at: Instant::now(),
                    });
                } else {
                    // TODO: real submission path — blockhash + sign + send bundle.
                    warn!("LIVE mode not yet implemented; forcing dry_run");
                }
            }
            _ = async {
                let mut rx = shutdown_rx.clone();
                rx.changed().await.ok();
            } => {
                if *shutdown_rx.borrow() {
                    info!("shutdown received, exiting after {} quote cycles", quote_count);
                    break;
                }
            }
        }
    }

    let _ = binance_handle.await;
    info!("manifest_mm exited cleanly");
    Ok(())
}

/// Pick a (mantissa, exponent) such that `value ≈ mantissa * 10^exponent`
/// with mantissa in u32 range and at least 6 significant figures.
///
/// Manifest encodes prices as u32 mantissa + i8 exponent. Returns (0, 0)
/// for non-finite or non-positive inputs.
fn encode_price_mantissa(value: f64) -> (u32, i8) {
    if !value.is_finite() || value <= 0.0 {
        return (0, 0);
    }
    // Aim for mantissa around 1e7-1e9 for 7-10 sig figs.
    let target = 1e8;
    let exp = (target / value).log10().round() as i32;
    let exp = exp.clamp(-20, 20);
    let mantissa_f = value * 10f64.powi(exp);
    let mantissa = mantissa_f.round().max(1.0) as u64;
    let mantissa_u32 = if mantissa > u32::MAX as u64 {
        u32::MAX
    } else {
        mantissa as u32
    };
    (mantissa_u32, -exp as i8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_price_round_trip() {
        for v in [0.42_f64, 100.0, 86.85, 1234.5678, 0.00001] {
            let (m, e) = encode_price_mantissa(v);
            let reconstructed = (m as f64) * 10f64.powi(e as i32);
            let err = (reconstructed - v).abs() / v;
            assert!(err < 1e-6, "roundtrip failed for {}: got {} (err {})", v, reconstructed, err);
        }
    }

    #[test]
    fn encode_price_rejects_bad_input() {
        assert_eq!(encode_price_mantissa(0.0), (0, 0));
        assert_eq!(encode_price_mantissa(-1.0), (0, 0));
        assert_eq!(encode_price_mantissa(f64::NAN), (0, 0));
        assert_eq!(encode_price_mantissa(f64::INFINITY), (0, 0));
    }
}
