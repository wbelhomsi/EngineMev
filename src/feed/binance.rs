//! Binance bookTicker WebSocket client for SOL/USDC (and future pairs).
//!
//! Pattern mirrors `src/state/tip_floor.rs` — auto-reconnect with backoff,
//! graceful shutdown, and first-message logging for debugging.

use anyhow::Result;
use futures::StreamExt;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use crate::feed::{PriceSnapshot, PriceStore};

/// Default Binance WebSocket endpoint.
pub const BINANCE_WS_URL: &str = "wss://stream.binance.com:9443/ws";

/// SOL/USDC bookTicker stream name.
pub const SOLUSDC_STREAM: &str = "solusdc@bookTicker";

/// Reconnect delay after disconnect.
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// If no message in this window, assume connection is dead and reconnect.
const WS_TIMEOUT: Duration = Duration::from_secs(30);

/// Connect to Binance bookTicker stream for SOL/USDC and update the PriceStore.
/// Reconnects on any error. Exits cleanly when `shutdown_rx` signals true.
pub async fn run_solusdc_loop(
    price_store: PriceStore,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        if *shutdown_rx.borrow() {
            break;
        }

        let url = format!("{}/{}", BINANCE_WS_URL, SOLUSDC_STREAM);
        info!("Connecting to Binance bookTicker: {}", url);

        let ws_result = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(&url),
        )
        .await;

        match ws_result {
            Ok(Ok((ws_stream, _response))) => {
                info!("Binance WS connected");
                let (_write, mut read) = ws_stream.split();
                let mut first_msg = true;

                loop {
                    tokio::select! {
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                info!("Binance WS loop shutting down");
                                return;
                            }
                        }
                        msg = tokio::time::timeout(WS_TIMEOUT, read.next()) => {
                            match msg {
                                Ok(Some(Ok(Message::Text(text)))) => {
                                    if first_msg {
                                        info!("Binance first message (raw): {}",
                                            &text[..text.len().min(500)]);
                                        first_msg = false;
                                    }
                                    match parse_book_ticker(&text) {
                                        Ok(snapshot) => {
                                            price_store.update_cex("SOLUSDC", snapshot);
                                        }
                                        Err(e) => {
                                            debug!("Failed to parse Binance msg: {}", e);
                                        }
                                    }
                                }
                                Ok(Some(Ok(Message::Ping(_)))) => {
                                    // tungstenite handles pong automatically
                                }
                                Ok(Some(Ok(Message::Close(_)))) => {
                                    warn!("Binance WS closed by server");
                                    break;
                                }
                                Ok(Some(Err(e))) => {
                                    warn!("Binance WS error: {}", e);
                                    break;
                                }
                                Ok(None) => {
                                    warn!("Binance WS ended");
                                    break;
                                }
                                Err(_) => {
                                    warn!("Binance WS no message in {}s, reconnecting",
                                        WS_TIMEOUT.as_secs());
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                warn!("Binance WS connect failed: {}", e);
            }
            Err(_) => {
                warn!("Binance WS connect timed out");
            }
        }

        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { break; }
            }
            _ = tokio::time::sleep(RECONNECT_DELAY) => {}
        }
    }

    info!("Binance WS loop exited");
}

/// Parse a bookTicker message into a PriceSnapshot.
///
/// Payload format:
/// ```json
/// { "u": 400900217, "s": "SOLUSDC", "b": "185.20", "B": "100.00",
///   "a": "185.21", "A": "50.00" }
/// ```
pub fn parse_book_ticker(text: &str) -> Result<PriceSnapshot> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    let best_bid: f64 = v["b"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'b'"))?
        .parse()?;
    let best_ask: f64 = v["a"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing 'a'"))?
        .parse()?;

    if best_bid <= 0.0 || best_ask <= 0.0 {
        anyhow::bail!("non-positive price: bid={} ask={}", best_bid, best_ask);
    }
    if best_bid > best_ask {
        anyhow::bail!("inverted book: bid={} ask={}", best_bid, best_ask);
    }

    Ok(PriceSnapshot {
        best_bid_usd: best_bid,
        best_ask_usd: best_ask,
        received_at: Instant::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_book_ticker_happy() {
        let msg = r#"{"u":400900217,"s":"SOLUSDC","b":"185.20","B":"100.00","a":"185.21","A":"50.00"}"#;
        let snap = parse_book_ticker(msg).unwrap();
        assert_eq!(snap.best_bid_usd, 185.20);
        assert_eq!(snap.best_ask_usd, 185.21);
        assert!((snap.mid() - 185.205).abs() < 1e-6);
    }

    #[test]
    fn test_parse_book_ticker_rejects_inverted() {
        let msg = r#"{"u":1,"s":"SOLUSDC","b":"185.25","B":"100","a":"185.20","A":"50"}"#;
        assert!(parse_book_ticker(msg).is_err());
    }

    #[test]
    fn test_parse_book_ticker_rejects_zero() {
        let msg = r#"{"u":1,"s":"SOLUSDC","b":"0","B":"100","a":"185.20","A":"50"}"#;
        assert!(parse_book_ticker(msg).is_err());
    }

    #[test]
    fn test_parse_book_ticker_rejects_missing_fields() {
        let msg = r#"{"u":1,"s":"SOLUSDC"}"#;
        assert!(parse_book_ticker(msg).is_err());
    }
}
