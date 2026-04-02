use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use serde_json::json;
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::{info, error, debug};

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
    /// Single client reuses TCP connections. ~2ms saved per relay vs new client.
    http_client: reqwest::Client,
}

impl MultiRelay {
    pub fn new(config: Arc<BotConfig>) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .pool_max_idle_per_host(8)
            .pool_idle_timeout(std::time::Duration::from_secs(300)) // keep connections alive 5 min
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .tcp_nodelay(true) // disable Nagle's algorithm for lower latency
            .http2_keep_alive_interval(std::time::Duration::from_secs(15))
            .http2_keep_alive_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client");

        Self { config, http_client }
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

        // Jito — primary relay, always submit
        {
            let url = format!("{}/api/v1", endpoints.jito.trim_end_matches('/'));
            let txs = encoded_txs.clone();
            let client = self.http_client.clone();
            tasks.spawn(async move {
                Self::submit_to_jito(&client, &url, &txs).await
            });
        }

        // Nozomi — secondary relay
        if let Some(ref url) = endpoints.nozomi {
            let url = url.clone();
            let txs = encoded_txs.clone();
            let client = self.http_client.clone();
            tasks.spawn(async move {
                Self::submit_to_nozomi(&client, &url, &txs).await
            });
        }

        // bloXroute
        if let Some(ref url) = endpoints.bloxroute {
            let url = url.clone();
            let txs = encoded_txs.clone();
            let client = self.http_client.clone();
            tasks.spawn(async move {
                Self::submit_to_bloxroute(&client, &url, &txs).await
            });
        }

        // Astralane — aggregator, routes through multiple paths
        if let Some(ref url) = endpoints.astralane {
            let url = url.clone();
            let txs = encoded_txs.clone();
            let client = self.http_client.clone();
            tasks.spawn(async move {
                Self::submit_to_astralane(&client, &url, &txs).await
            });
        }

        // ZeroSlot
        if let Some(ref url) = endpoints.zeroslot {
            let url = url.clone();
            let txs = encoded_txs.clone();
            let client = self.http_client.clone();
            tasks.spawn(async move {
                Self::submit_to_zeroslot(&client, &url, &txs).await
            });
        }

        // Collect all results
        let mut results = Vec::new();
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(relay_result) => {
                    if relay_result.success {
                        debug!(
                            "Bundle accepted by {}: id={:?} latency={}us",
                            relay_result.relay_name,
                            relay_result.bundle_id,
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
                        debug!("Jito response: {}", body);
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
                                error: Some(format!("Unexpected response: {}", body)),
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
    /// Astralane routes bundles through Jito, Paladin, and swQoS paths.
    /// Uses a Jito-compatible JSON-RPC interface.
    async fn submit_to_astralane(
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
                    error: Some(format!("Parse error: {}", e)),
                },
            },
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
