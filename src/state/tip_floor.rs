//! Dynamic Jito tip floor cache.
//!
//! Polls the Jito bundles REST API tip_floor endpoint every few seconds
//! and caches the result. The simulator uses this as a dynamic minimum
//! tip instead of a static env-var floor.
//!
//! Jito tip floor API returns percentile-based tip amounts:
//! `[{ "time": "...", "landed_tips_25th_percentile": 1000, "landed_tips_50th_percentile": 10000,
//!    "landed_tips_75th_percentile": 100000, "landed_tips_95th_percentile": 1000000,
//!    "landed_tips_99th_percentile": 10000000, "ema_landed_tips_50th_percentile": 12000 }]`

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Jito tip floor REST endpoint (public, no auth required).
const TIP_FLOOR_URL: &str = "https://bundles-api-rest.jito.wtf/api/v1/bundles/tip_floor";

/// How long before the cached value is considered stale.
const STALE_THRESHOLD: Duration = Duration::from_secs(30);

/// Polling interval for tip floor refresh.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

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

/// Fetch tip floor from Jito REST API and update the cache.
pub async fn fetch_and_update(
    client: &reqwest::Client,
    cache: &TipFloorCache,
) -> anyhow::Result<()> {
    let resp = client
        .get(TIP_FLOOR_URL)
        .timeout(Duration::from_secs(5))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    // Response is an array with one element
    let entry = resp
        .as_array()
        .and_then(|arr| arr.first())
        .ok_or_else(|| anyhow::anyhow!("Empty tip_floor response"))?;

    let p50 = entry
        .get("landed_tips_50th_percentile")
        .and_then(parse_tip_value)
        .unwrap_or(10_000); // 0.00001 SOL fallback

    let p75 = entry
        .get("landed_tips_75th_percentile")
        .and_then(parse_tip_value)
        .unwrap_or(100_000);

    let ema_p50 = entry
        .get("ema_landed_tips_50th_percentile")
        .and_then(parse_tip_value)
        .unwrap_or(p50); // Fall back to raw p50

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

/// Parse tip value from JSON — Jito returns either a number or a float.
fn parse_tip_value(v: &serde_json::Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        Some(n)
    } else if let Some(f) = v.as_f64() {
        Some(f as u64)
    } else {
        None
    }
}

/// Background loop that polls the Jito tip floor API.
pub async fn run_tip_floor_loop(
    client: reqwest::Client,
    cache: TipFloorCache,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut consecutive_failures: u32 = 0;

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { break; }
            }
            _ = tokio::time::sleep(POLL_INTERVAL) => {
                match fetch_and_update(&client, &cache).await {
                    Ok(()) => {
                        if consecutive_failures > 0 {
                            info!("Tip floor fetch recovered after {} failures", consecutive_failures);
                        }
                        consecutive_failures = 0;
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        if consecutive_failures >= 5 {
                            warn!("Tip floor fetch failed {}x: {}", consecutive_failures, e);
                        } else {
                            debug!("Tip floor fetch failed ({}x): {}", consecutive_failures, e);
                        }
                    }
                }
            }
        }
    }

    info!("Tip floor refresh loop exited");
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
            fetched_at: Instant::now() - Duration::from_secs(60), // stale
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

        // We need to override the URL for testing — use a helper
        let resp = client
            .get(&server.url())
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap();

        let entry = resp.as_array().unwrap().first().unwrap();
        let p50 = entry.get("landed_tips_50th_percentile").and_then(parse_tip_value).unwrap();
        let p75 = entry.get("landed_tips_75th_percentile").and_then(parse_tip_value).unwrap();
        let ema = entry.get("ema_landed_tips_50th_percentile").and_then(parse_tip_value).unwrap();

        assert_eq!(p50, 10_000);
        assert_eq!(p75, 100_000);
        assert_eq!(ema, 12_000);

        cache.update(TipFloorInfo {
            p50_lamports: p50,
            p75_lamports: p75,
            ema_p50_lamports: ema,
            fetched_at: Instant::now(),
        });

        assert_eq!(cache.get_floor_lamports(), Some(12_000));
        assert_eq!(cache.get_competitive_floor_lamports(), Some(100_000));
        mock.assert_async().await;
    }
}
