use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use serde_json::json;
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::{info, warn, error, debug};

use crate::config::BotConfig;

/// Relay submission result for tracking which relay landed the bundle.
#[derive(Debug)]
pub struct RelayResult {
    pub relay_name: String,
    pub bundle_id: Option<String>,
    pub success: bool,
    pub latency_us: u64,
    pub error: Option<String>,
}

/// Multi-relay fan-out submission layer.
///
/// Submits the same bundle to all configured relays simultaneously.
/// Since Jito bundles are atomic, only one will land — the rest are no-ops.
/// This maximizes inclusion probability across different validator sets.
///
/// Architecture:
/// - Fan-out is fully concurrent (tokio::JoinSet)
/// - Each relay gets its own async task
/// - Results are collected for metrics/logging
/// - Tip adjustment per relay is supported (competitive auctions differ)
///
/// Relay APIs (current as of 2026):
/// - Jito: JSON-RPC via `jito-sdk-rust` (replaces deprecated gRPC SearcherServiceClient)
/// - Nozomi, bloXroute, ZeroSlot, Astralane: REST/JSON-RPC with base64-encoded txs
pub struct MultiRelay {
    config: Arc<BotConfig>,
    /// Shared HTTP client — connection pooling across relay calls.
    http_client: reqwest::Client,
    /// Per-relay rate limiters: relay_name → last submission time
    last_submit: Arc<dashmap::DashMap<String, std::time::Instant>>,
}

/// Per-relay rate limits from env (e.g., JITO_TPS=1, ASTRALANE_TPS=40).
/// Falls back to sensible defaults if not set.
fn relay_rate_limit(name: &str) -> std::time::Duration {
    let env_key = format!("{}_TPS", name.to_uppercase());
    let tps: f64 = std::env::var(&env_key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(match name {
            "jito" => 1.0,
            "astralane" => 40.0,
            "nozomi" => 5.0,
            "bloxroute" => 5.0,
            "zeroslot" => 5.0,
            _ => 1.0,
        });
    if tps <= 0.0 {
        return std::time::Duration::from_millis(1000);
    }
    std::time::Duration::from_millis((1000.0 / tps) as u64 + 10) // +10ms safety margin
}

impl MultiRelay {
    pub fn new(config: Arc<BotConfig>) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .pool_max_idle_per_host(8)
            .pool_idle_timeout(std::time::Duration::from_secs(300))
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .tcp_nodelay(true)
            .http2_keep_alive_interval(std::time::Duration::from_secs(15))
            .http2_keep_alive_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");

        Self { config, http_client, last_submit: Arc::new(dashmap::DashMap::new()) }
    }

    /// Check if a relay is ready to accept a submission (not rate limited).
    fn can_submit(&self, relay_name: &str) -> bool {
        let limit = relay_rate_limit(relay_name);
        match self.last_submit.get(relay_name) {
            Some(last) => last.value().elapsed() >= limit,
            None => true,
        }
    }

    /// Mark a relay as having just submitted.
    fn mark_submitted(&self, relay_name: &str) {
        self.last_submit.insert(relay_name.to_string(), std::time::Instant::now());
    }

    /// Spawn a background keepalive loop for Astralane.
    /// Pings getHealth every 5s to keep the TCP connection hot and avoid cold-start latency.
    pub fn spawn_astralane_keepalive(&self, shutdown_rx: tokio::sync::watch::Receiver<bool>) {
        if let Some(ref url) = self.config.relay_endpoints.astralane {
            let client = self.http_client.clone();
            let api_key = std::env::var("ASTRALANE_API_KEY").unwrap_or_default();
            let url = url.clone();
            let mut shutdown = shutdown_rx;
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = shutdown.changed() => {
                            if *shutdown.borrow() { break; }
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                            let payload = serde_json::json!({
                                "jsonrpc": "2.0", "id": 1, "method": "getHealth"
                            });
                            let _ = client
                                .post(&url)
                                .header("api_key", &api_key)
                                .json(&payload)
                                .send()
                                .await;
                        }
                    }
                }
            });
        }
    }

    /// Warm up connections to all configured relays.
    /// Call at startup to pre-establish TCP+TLS+HTTP2 connections
    /// so the first bundle submission doesn't pay cold-connect latency.
    pub async fn warmup(&self) {
        let endpoints = &self.config.relay_endpoints;

        // Jito tip floor API is a lightweight GET that warms the connection
        let url = format!("{}/api/v1/bundles/tip_floor", endpoints.jito.trim_end_matches('/'));
        match self.http_client.get(&url).send().await {
            Ok(_) => info!("Relay warmup: Jito connection established"),
            Err(e) => debug!("Relay warmup: Jito ping failed ({}), will connect on first bundle", e),
        }

        // Warm other relays if configured
        for (name, endpoint) in [
            ("nozomi", &endpoints.nozomi),
            ("bloxroute", &endpoints.bloxroute),
            ("astralane", &endpoints.astralane),
            ("zeroslot", &endpoints.zeroslot),
        ] {
            if let Some(url) = endpoint {
                match self.http_client.get(url.as_str()).send().await {
                    Ok(_) => info!("Relay warmup: {} connection established", name),
                    Err(_) => debug!("Relay warmup: {} ping failed", name),
                }
            }
        }
    }

    /// Submit a bundle to all configured relays concurrently.
    ///
    /// `bundle_txs` - serialized transactions forming the bundle (bincode-encoded)
    /// `tip_lamports` - base tip (may be adjusted per relay)
    ///
    /// Returns results from all relay submissions.
    pub async fn submit_bundle(
        &self,
        bundle_txs: &[Vec<u8>],
        tip_lamports: u64,
    ) -> Vec<RelayResult> {
        // Pre-encode all txs to base64 once — shared across relays.
        let encoded_txs: Vec<String> = bundle_txs
            .iter()
            .map(|tx| general_purpose::STANDARD.encode(tx))
            .collect();

        let mut tasks = JoinSet::new();
        let endpoints = &self.config.relay_endpoints;

        // Jito — primary relay (rate limited: 1/sec unauth)
        if self.can_submit("jito") {
            let url = format!("{}/api/v1", endpoints.jito.trim_end_matches('/'));
            let txs = encoded_txs.clone();
            let client = self.http_client.clone();
            self.mark_submitted("jito");
            tasks.spawn(async move {
                Self::submit_to_jito(&client, &url, &txs).await
            });
        }

        // Nozomi — secondary relay
        if let Some(ref url) = endpoints.nozomi {
            if self.can_submit("nozomi") {
                let url = url.clone();
                let txs = encoded_txs.clone();
                let client = self.http_client.clone();
                self.mark_submitted("nozomi");
                tasks.spawn(async move {
                    Self::submit_to_nozomi(&client, &url, &txs).await
                });
            }
        }

        // bloXroute
        if let Some(ref url) = endpoints.bloxroute {
            if self.can_submit("bloxroute") {
                let url = url.clone();
                let txs = encoded_txs.clone();
                let client = self.http_client.clone();
                self.mark_submitted("bloxroute");
                tasks.spawn(async move {
                    Self::submit_to_bloxroute(&client, &url, &txs).await
                });
            }
        }

        // Astralane — 40 TPS, revert_protect
        if let Some(ref url) = endpoints.astralane {
            if self.can_submit("astralane") {
                let url = url.clone();
                let txs = encoded_txs.clone();
                let client = self.http_client.clone();
                self.mark_submitted("astralane");
                tasks.spawn(async move {
                    Self::submit_to_astralane(&client, &url, &txs).await
                });
            }
        }

        // ZeroSlot
        if let Some(ref url) = endpoints.zeroslot {
            if self.can_submit("zeroslot") {
                let url = url.clone();
                let txs = encoded_txs.clone();
                let client = self.http_client.clone();
                self.mark_submitted("zeroslot");
                tasks.spawn(async move {
                    Self::submit_to_zeroslot(&client, &url, &txs).await
                });
            }
        }

        // Collect all results
        let mut results = Vec::new();
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(relay_result) => {
                    if relay_result.success {
                        info!(
                            "Bundle accepted by {}: id={:?} latency={}us",
                            relay_result.relay_name,
                            relay_result.bundle_id,
                            relay_result.latency_us,
                        );
                    } else {
                        warn!(
                            "Bundle REJECTED by {}: {:?} (latency={}us)",
                            relay_result.relay_name,
                            relay_result.error,
                            relay_result.latency_us,
                        );
                    }
                    results.push(relay_result);
                }
                Err(e) => {
                    error!("Relay task panicked: {}", e);
                }
            }
        }

        let accepted = results.iter().filter(|r| r.success).count();
        info!(
            "Bundle submitted to {} relays, {} accepted",
            results.len(),
            accepted
        );

        results
    }

    /// Submit to Jito block engine via JSON-RPC `sendBundle`.
    ///
    /// Uses the jito-sdk-rust JSON-RPC interface (v0.2+).
    /// The old gRPC SearcherServiceClient was deprecated — this is the current path.
    ///
    /// Endpoint: POST {block_engine_url}/api/v1/bundles
    /// Payload: JSON-RPC 2.0 with method "sendBundle"
    /// Params: [[base64_tx1, base64_tx2, ...], {"encoding": "base64"}]
    async fn submit_to_jito(
        client: &reqwest::Client,
        base_url: &str,
        encoded_txs: &[String],
    ) -> RelayResult {
        let start = std::time::Instant::now();

        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [
                encoded_txs,
                { "encoding": "base64" }
            ]
        });

        // Jito auth UUID from env (optional, gives priority in auction)
        let mut req = client
            .post(format!("{}/bundles", base_url))
            .json(&payload);

        if let Ok(auth_uuid) = std::env::var("JITO_AUTH_UUID") {
            if !auth_uuid.is_empty() {
                req = req.header("x-jito-auth", &auth_uuid);
            }
        }

        let result = req.send().await;

        let latency = start.elapsed().as_micros() as u64;

        match result {
            Ok(resp) => {
                match resp.json::<serde_json::Value>().await {
                    Ok(body) => {
                        if let Some(bundle_id) = body.get("result").and_then(|v| v.as_str()) {
                            RelayResult {
                                relay_name: "jito".to_string(),
                                bundle_id: Some(bundle_id.to_string()),
                                success: true,
                                latency_us: latency,
                                error: None,
                            }
                        } else if let Some(err) = body.get("error") {
                            RelayResult {
                                relay_name: "jito".to_string(),
                                bundle_id: None,
                                success: false,
                                latency_us: latency,
                                error: Some(format!("{}", err)),
                            }
                        } else {
                            RelayResult {
                                relay_name: "jito".to_string(),
                                bundle_id: None,
                                success: false,
                                latency_us: latency,
                                error: Some("Unexpected response format".to_string()),
                            }
                        }
                    }
                    Err(e) => RelayResult {
                        relay_name: "jito".to_string(),
                        bundle_id: None,
                        success: false,
                        latency_us: latency,
                        error: Some(format!("Response parse error: {}", e)),
                    },
                }
            }
            Err(e) => RelayResult {
                relay_name: "jito".to_string(),
                bundle_id: None,
                success: false,
                latency_us: latency,
                error: Some(format!("Request failed: {}", e)),
            },
        }
    }

    /// Submit to Nozomi relay via JSON-RPC sendBundle.
    ///
    /// Nozomi exposes a Jito-compatible JSON-RPC interface.
    /// Same sendBundle method, same base64 encoding.
    async fn submit_to_nozomi(
        client: &reqwest::Client,
        url: &str,
        encoded_txs: &[String],
    ) -> RelayResult {
        let start = std::time::Instant::now();

        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [encoded_txs]
        });

        let result = client
            .post(url)
            .json(&payload)
            .send()
            .await;

        let latency = start.elapsed().as_micros() as u64;

        match result {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(body) => {
                    let bundle_id = body.get("result").and_then(|v| v.as_str()).map(String::from);
                    let success = bundle_id.is_some();
                    let error = if !success {
                        body.get("error").map(|e| format!("{}", e))
                    } else {
                        None
                    };
                    RelayResult {
                        relay_name: "nozomi".to_string(),
                        bundle_id,
                        success,
                        latency_us: latency,
                        error,
                    }
                }
                Err(e) => RelayResult {
                    relay_name: "nozomi".to_string(),
                    bundle_id: None,
                    success: false,
                    latency_us: latency,
                    error: Some(format!("Parse error: {}", e)),
                },
            },
            Err(e) => RelayResult {
                relay_name: "nozomi".to_string(),
                bundle_id: None,
                success: false,
                latency_us: latency,
                error: Some(format!("Request failed: {}", e)),
            },
        }
    }

    /// Submit to bloXroute via REST.
    ///
    /// bloXroute Solana bundle submission:
    /// POST /api/v2/submit-bundle with base64 transactions and auth header.
    async fn submit_to_bloxroute(
        client: &reqwest::Client,
        url: &str,
        encoded_txs: &[String],
    ) -> RelayResult {
        let start = std::time::Instant::now();

        let payload = json!({
            "transaction": encoded_txs,
            "useBundle": true,
        });

        // bloXroute requires Authorization header with API key (from env)
        let auth_key = std::env::var("BLOXROUTE_AUTH_HEADER").unwrap_or_default();

        let result = client
            .post(format!("{}/api/v2/submit-bundle", url.trim_end_matches('/')))
            .header("Authorization", &auth_key)
            .json(&payload)
            .send()
            .await;

        let latency = start.elapsed().as_micros() as u64;

        match result {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(body) => {
                    let bundle_id = body.get("bundleId").and_then(|v| v.as_str()).map(String::from);
                    let success = bundle_id.is_some();
                    let error = if !success {
                        Some(format!("{}", body))
                    } else {
                        None
                    };
                    RelayResult {
                        relay_name: "bloxroute".to_string(),
                        bundle_id,
                        success,
                        latency_us: latency,
                        error,
                    }
                }
                Err(e) => RelayResult {
                    relay_name: "bloxroute".to_string(),
                    bundle_id: None,
                    success: false,
                    latency_us: latency,
                    error: Some(format!("Parse error: {}", e)),
                },
            },
            Err(e) => RelayResult {
                relay_name: "bloxroute".to_string(),
                bundle_id: None,
                success: false,
                latency_us: latency,
                error: Some(format!("Request failed: {}", e)),
            },
        }
    }

    /// Submit to Astralane Iris aggregator.
    ///
    /// Uses Jito-compatible sendBundle format with revertProtection option.
    /// Auth via api_key header OR ?api-key= query param.
    /// Response: {"result": ["tx_sig_1", ...]} on success.
    async fn submit_to_astralane(
        client: &reqwest::Client,
        url: &str,
        encoded_txs: &[String],
    ) -> RelayResult {
        let start = std::time::Instant::now();

        // Jito-compatible sendBundle format
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [
                encoded_txs,
                {
                    "encoding": "base64",
                    "revertProtection": true
                }
            ]
        });

        let api_key = std::env::var("ASTRALANE_API_KEY").unwrap_or_default();

        // Auth: try both query param and header (docs say both work)
        let url_with_auth = if !api_key.is_empty() {
            if url.contains('?') {
                format!("{}&api-key={}", url, api_key)
            } else {
                format!("{}?api-key={}", url, api_key)
            }
        } else {
            url.to_string()
        };

        let result = client
            .post(&url_with_auth)
            .header("api_key", &api_key)
            .json(&payload)
            .send()
            .await;

        let latency = start.elapsed().as_micros() as u64;

        match result {
            Ok(resp) => {
                let status = resp.status();
                match resp.text().await {
                    Ok(text) => {
                        if !status.is_success() {
                            warn!("Astralane HTTP {}: {}", status, &text[..text.len().min(200)]);
                        }
                        match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(body) => {
                    debug!("Astralane response: {}", body);
                    // Astralane returns {"result": ["tx_sig_1", ...]} on success
                    let bundle_id = body.get("result")
                        .and_then(|v| {
                            // Array of signatures
                            if let Some(arr) = v.as_array() {
                                arr.first().and_then(|s| s.as_str()).map(String::from)
                            } else if let Some(s) = v.as_str() {
                                Some(s.to_string())
                            } else if !v.is_null() {
                                Some(format!("{}", v))
                            } else {
                                None
                            }
                        });
                    let success = bundle_id.is_some();
                    let error = if !success {
                        if let Some(err) = body.get("error") {
                            Some(format!("{}", err))
                        } else {
                            Some(format!("Astralane: {}", body))
                        }
                    } else {
                        None
                    };
                    RelayResult {
                        relay_name: "astralane".to_string(),
                        bundle_id,
                        success,
                        latency_us: latency,
                        error,
                    }
                }
                            Err(e) => RelayResult {
                                relay_name: "astralane".to_string(),
                                bundle_id: None,
                                success: false,
                                latency_us: latency,
                                error: Some(format!("JSON parse error: {} (raw: {})", e, &text[..text.len().min(200)])),
                            },
                        }
                    }
                    Err(e) => RelayResult {
                        relay_name: "astralane".to_string(),
                        bundle_id: None,
                        success: false,
                        latency_us: latency,
                        error: Some(format!("Body read error: {}", e)),
                    },
                }
            }
            Err(e) => RelayResult {
                relay_name: "astralane".to_string(),
                bundle_id: None,
                success: false,
                latency_us: latency,
                error: Some(format!("Request failed: {}", e)),
            },
        }
    }

    /// Submit to ZeroSlot relay.
    ///
    /// ZeroSlot uses a Jito-compatible JSON-RPC sendBundle interface.
    async fn submit_to_zeroslot(
        client: &reqwest::Client,
        url: &str,
        encoded_txs: &[String],
    ) -> RelayResult {
        let start = std::time::Instant::now();

        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [encoded_txs]
        });

        let result = client
            .post(url)
            .json(&payload)
            .send()
            .await;

        let latency = start.elapsed().as_micros() as u64;

        match result {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(body) => {
                    let bundle_id = body.get("result").and_then(|v| v.as_str()).map(String::from);
                    let success = bundle_id.is_some();
                    let error = if !success {
                        body.get("error").map(|e| format!("{}", e))
                    } else {
                        None
                    };
                    RelayResult {
                        relay_name: "zeroslot".to_string(),
                        bundle_id,
                        success,
                        latency_us: latency,
                        error,
                    }
                }
                Err(e) => RelayResult {
                    relay_name: "zeroslot".to_string(),
                    bundle_id: None,
                    success: false,
                    latency_us: latency,
                    error: Some(format!("Parse error: {}", e)),
                },
            },
            Err(e) => RelayResult {
                relay_name: "zeroslot".to_string(),
                bundle_id: None,
                success: false,
                latency_us: latency,
                error: Some(format!("Request failed: {}", e)),
            },
        }
    }
}
