//! Dynamic Jito tip floor via WebSocket stream.
//!
//! Connects to `wss://bundles.jito.wtf/api/v1/bundles/tip_stream` and receives
//! real-time tip floor updates pushed by Jito. Falls back to REST polling if
//! the WebSocket connection fails.
//!
//! Jito tip stream pushes JSON messages:
//! `[{ "time": "...", "landed_tips_25th_percentile": 1000, "landed_tips_50th_percentile": 10000,
//!    "landed_tips_75th_percentile": 100000, "landed_tips_95th_percentile": 1000000,
//!    "landed_tips_99th_percentile": 10000000, "ema_landed_tips_50th_percentile": 12000 }]`

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Jito tip stream WebSocket endpoint.
const TIP_STREAM_WS_URL: &str = "wss://bundles.jito.wtf/api/v1/bundles/tip_stream";

/// Jito tip floor REST endpoint (fallback).
const TIP_FLOOR_REST_URL: &str = "https://bundles-api-rest.jito.wtf/api/v1/bundles/tip_floor";

/// How long before the cached value is considered stale.
const STALE_THRESHOLD: Duration = Duration::from_secs(30);

/// Delay before reconnecting after a WebSocket disconnect.
const WS_RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// If no WS message received within this duration, reconnect.
const WS_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct TipFloorInfo {
    /// 50th percentile of recent landed tips (in lamports).
    pub p50_lamports: u64,
    /// 75th percentile — competitive floor for most opportunities.
    pub p75_lamports: u64,
    /// EMA of 50th percentile — smoothed signal, less noisy.
    pub ema_p50_lamports: u64,
    /// When this data was fetched.
    pub fetched_at: Instant,
}

#[derive(Clone)]
pub struct TipFloorCache {
    inner: Arc<RwLock<Option<TipFloorInfo>>>,
}

impl Default for TipFloorCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TipFloorCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
        }
    }

    pub fn update(&self, info: TipFloorInfo) {
        match self.inner.write() {
            Ok(mut guard) => *guard = Some(info),
            Err(poisoned) => *poisoned.into_inner() = Some(info),
        }
    }

    /// Get the dynamic tip floor (EMA of 50th percentile).
    /// Returns None if data is stale or never fetched.
    pub fn get_floor_lamports(&self) -> Option<u64> {
        let guard = match self.inner.read() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.as_ref().and_then(|info| {
            if info.fetched_at.elapsed() < STALE_THRESHOLD {
                Some(info.ema_p50_lamports)
            } else {
                None
            }
        })
    }

    /// Get the competitive tip floor (75th percentile).
    /// Returns None if data is stale or never fetched.
    pub fn get_competitive_floor_lamports(&self) -> Option<u64> {
        let guard = match self.inner.read() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.as_ref().and_then(|info| {
            if info.fetched_at.elapsed() < STALE_THRESHOLD {
                Some(info.p75_lamports)
            } else {
                None
            }
        })
    }
}

/// Parse a tip floor JSON message (array with one entry) and update the cache.
/// Used by both WS stream and REST fallback.
pub fn parse_and_update(json: &serde_json::Value, cache: &TipFloorCache) -> anyhow::Result<()> {
    let entry = json
        .as_array()
        .and_then(|arr| arr.first())
        .ok_or_else(|| anyhow::anyhow!("Empty tip_floor response"))?;

    let p50 = entry
        .get("landed_tips_50th_percentile")
        .and_then(parse_tip_value)
        .unwrap_or(10_000);

    let p75 = entry
        .get("landed_tips_75th_percentile")
        .and_then(parse_tip_value)
        .unwrap_or(100_000);

    let ema_p50 = entry
        .get("ema_landed_tips_50th_percentile")
        .and_then(parse_tip_value)
        .unwrap_or(p50);

    cache.update(TipFloorInfo {
        p50_lamports: p50,
        p75_lamports: p75,
        ema_p50_lamports: ema_p50,
        fetched_at: Instant::now(),
    });

    debug!(
        "Tip floor updated: p50={} p75={} ema_p50={} lamports",
        p50, p75, ema_p50
    );

    Ok(())
}

/// Fetch tip floor from Jito REST API and update the cache.
pub async fn fetch_and_update(
    client: &reqwest::Client,
    cache: &TipFloorCache,
) -> anyhow::Result<()> {
    let resp = client
        .get(TIP_FLOOR_REST_URL)
        .timeout(Duration::from_secs(5))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    parse_and_update(&resp, cache)
}

/// Parse tip value from JSON — Jito may return lamports (integer) or SOL (float).
/// Values < 1000 are assumed to be SOL and converted to lamports.
fn parse_tip_value(v: &serde_json::Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        Some(n)
    } else if let Some(f) = v.as_f64() {
        if f > 0.0 && f < 1000.0 {
            // Likely SOL, convert to lamports
            Some((f * 1_000_000_000.0) as u64)
        } else {
            Some(f as u64)
        }
    } else {
        None
    }
}

/// Background loop that connects to Jito tip stream WebSocket.
/// Falls back to REST polling if WS connection fails.
pub async fn run_tip_floor_loop(
    client: reqwest::Client,
    cache: TipFloorCache,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::Message;

    loop {
        // Check shutdown before each connection attempt
        if *shutdown_rx.borrow() {
            break;
        }

        info!("Connecting to Jito tip stream WebSocket...");

        let ws_result = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(TIP_STREAM_WS_URL),
        )
        .await;

        match ws_result {
            Ok(Ok((ws_stream, _response))) => {
                info!("Jito tip stream WebSocket connected");
                let (_write, mut read) = ws_stream.split();
                let mut first_msg = true;

                loop {
                    tokio::select! {
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                info!("Tip floor WS loop shutting down");
                                return;
                            }
                        }
                        msg = tokio::time::timeout(WS_TIMEOUT, read.next()) => {
                            match msg {
                                Ok(Some(Ok(Message::Text(text)))) => {
                                    if first_msg {
                                        info!("Tip stream first message (raw): {}", &text[..text.len().min(500)]);
                                        first_msg = false;
                                    }
                                    match serde_json::from_str::<serde_json::Value>(&text) {
                                        Ok(json) => {
                                            // WS may send a single object or an array
                                            let as_array = if json.is_array() {
                                                json
                                            } else {
                                                serde_json::Value::Array(vec![json])
                                            };
                                            if let Err(e) = parse_and_update(&as_array, &cache) {
                                                debug!("Failed to parse tip stream message: {}", e);
                                            }
                                        }
                                        Err(e) => {
                                            debug!("Invalid JSON from tip stream: {}", e);
                                        }
                                    }
                                }
                                Ok(Some(Ok(Message::Ping(_)))) => {
                                    // tungstenite handles pong automatically
                                }
                                Ok(Some(Ok(Message::Close(_)))) => {
                                    warn!("Jito tip stream WebSocket closed by server");
                                    break;
                                }
                                Ok(Some(Err(e))) => {
                                    warn!("Jito tip stream WebSocket error: {}", e);
                                    break;
                                }
                                Ok(None) => {
                                    warn!("Jito tip stream WebSocket ended");
                                    break;
                                }
                                Err(_) => {
                                    warn!("Jito tip stream no message in {}s, reconnecting", WS_TIMEOUT.as_secs());
                                    break;
                                }
                                _ => {} // Binary, Frame — ignore
                            }
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                warn!("Jito tip stream WS connect failed: {}, falling back to REST", e);
                // One-shot REST fallback before retry
                if let Err(rest_err) = fetch_and_update(&client, &cache).await {
                    debug!("REST fallback also failed: {}", rest_err);
                }
            }
            Err(_) => {
                warn!("Jito tip stream WS connect timed out, falling back to REST");
                if let Err(rest_err) = fetch_and_update(&client, &cache).await {
                    debug!("REST fallback also failed: {}", rest_err);
                }
            }
        }

        // Wait before reconnecting (check shutdown)
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { break; }
            }
            _ = tokio::time::sleep(WS_RECONNECT_DELAY) => {}
        }
    }

    info!("Tip floor stream loop exited");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tip_floor_cache_default_is_none() {
        let cache = TipFloorCache::new();
        assert!(cache.get_floor_lamports().is_none());
        assert!(cache.get_competitive_floor_lamports().is_none());
    }

    #[test]
    fn test_tip_floor_cache_returns_value() {
        let cache = TipFloorCache::new();
        cache.update(TipFloorInfo {
            p50_lamports: 10_000,
            p75_lamports: 100_000,
            ema_p50_lamports: 12_000,
            fetched_at: Instant::now(),
        });
        assert_eq!(cache.get_floor_lamports(), Some(12_000));
        assert_eq!(cache.get_competitive_floor_lamports(), Some(100_000));
    }

    #[test]
    fn test_tip_floor_cache_stale_returns_none() {
        let cache = TipFloorCache::new();
        cache.update(TipFloorInfo {
            p50_lamports: 10_000,
            p75_lamports: 100_000,
            ema_p50_lamports: 12_000,
            fetched_at: Instant::now() - Duration::from_secs(60),
        });
        assert!(cache.get_floor_lamports().is_none());
    }

    #[test]
    fn test_parse_tip_value_integer() {
        let v = serde_json::json!(50000);
        assert_eq!(parse_tip_value(&v), Some(50000));
    }

    #[test]
    fn test_parse_tip_value_float() {
        let v = serde_json::json!(50000.5);
        assert_eq!(parse_tip_value(&v), Some(50000));
    }

    #[test]
    fn test_parse_tip_value_null() {
        let v = serde_json::json!(null);
        assert_eq!(parse_tip_value(&v), None);
    }

    #[test]
    fn test_parse_and_update_from_json() {
        let cache = TipFloorCache::new();
        let json = serde_json::json!([{
            "time": "2024-01-01T00:00:00Z",
            "landed_tips_25th_percentile": 1000,
            "landed_tips_50th_percentile": 10000,
            "landed_tips_75th_percentile": 100000,
            "landed_tips_95th_percentile": 1000000,
            "landed_tips_99th_percentile": 10000000,
            "ema_landed_tips_50th_percentile": 12000
        }]);

        parse_and_update(&json, &cache).unwrap();
        assert_eq!(cache.get_floor_lamports(), Some(12_000));
        assert_eq!(cache.get_competitive_floor_lamports(), Some(100_000));
    }

    #[test]
    fn test_parse_and_update_empty_array() {
        let cache = TipFloorCache::new();
        let json = serde_json::json!([]);
        assert!(parse_and_update(&json, &cache).is_err());
    }

    #[tokio::test]
    async fn test_fetch_and_update_mock() {
        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("GET", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"[{
                "time": "2024-01-01T00:00:00Z",
                "landed_tips_25th_percentile": 1000,
                "landed_tips_50th_percentile": 10000,
                "landed_tips_75th_percentile": 100000,
                "landed_tips_95th_percentile": 1000000,
                "landed_tips_99th_percentile": 10000000,
                "ema_landed_tips_50th_percentile": 12000
            }]"#)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let cache = TipFloorCache::new();

        let resp = client
            .get(&server.url())
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap();

        parse_and_update(&resp, &cache).unwrap();
        assert_eq!(cache.get_floor_lamports(), Some(12_000));
        assert_eq!(cache.get_competitive_floor_lamports(), Some(100_000));
        mock.assert_async().await;
    }
}
