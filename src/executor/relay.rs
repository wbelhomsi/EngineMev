use anyhow::Result;
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::{info, warn, error, debug};

use crate::config::{BotConfig, RelayEndpoints};

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
pub struct MultiRelay {
    config: Arc<BotConfig>,
}

impl MultiRelay {
    pub fn new(config: Arc<BotConfig>) -> Self {
        Self { config }
    }

    /// Submit a bundle to all configured relays concurrently.
    ///
    /// `bundle_txs` - serialized transactions forming the bundle
    /// `tip_lamports` - base tip (may be adjusted per relay)
    ///
    /// Returns results from all relay submissions.
    pub async fn submit_bundle(
        &self,
        bundle_txs: &[Vec<u8>],
        tip_lamports: u64,
    ) -> Vec<RelayResult> {
        let mut tasks = JoinSet::new();
        let endpoints = &self.config.relay_endpoints;

        // Jito — primary relay, always submit
        {
            let url = endpoints.jito.clone();
            let txs = bundle_txs.to_vec();
            let tip = tip_lamports;
            tasks.spawn(async move {
                Self::submit_to_jito(&url, &txs, tip).await
            });
        }

        // Nozomi — secondary relay
        if let Some(ref url) = endpoints.nozomi {
            let url = url.clone();
            let txs = bundle_txs.to_vec();
            // Nozomi auctions are thinner — lower tip can still win
            let tip = (tip_lamports as f64 * 0.85) as u64;
            tasks.spawn(async move {
                Self::submit_to_nozomi(&url, &txs, tip).await
            });
        }

        // bloXroute
        if let Some(ref url) = endpoints.bloxroute {
            let url = url.clone();
            let txs = bundle_txs.to_vec();
            let tip = (tip_lamports as f64 * 0.90) as u64;
            tasks.spawn(async move {
                Self::submit_to_bloxroute(&url, &txs, tip).await
            });
        }

        // Astralane — aggregator, routes through multiple paths
        if let Some(ref url) = endpoints.astralane {
            let url = url.clone();
            let txs = bundle_txs.to_vec();
            let tip = tip_lamports; // full tip since it routes through Jito internally
            tasks.spawn(async move {
                Self::submit_to_astralane(&url, &txs, tip).await
            });
        }

        // ZeroSlot
        if let Some(ref url) = endpoints.zeroslot {
            let url = url.clone();
            let txs = bundle_txs.to_vec();
            let tip = (tip_lamports as f64 * 0.80) as u64;
            tasks.spawn(async move {
                Self::submit_to_zeroslot(&url, &txs, tip).await
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

    /// Submit to Jito block engine via gRPC.
    async fn submit_to_jito(
        url: &str,
        bundle_txs: &[Vec<u8>],
        tip_lamports: u64,
    ) -> RelayResult {
        let start = std::time::Instant::now();

        // TODO: Actual Jito gRPC bundle submission
        // let channel = tonic::transport::Channel::from_shared(url.to_string())
        //     .unwrap()
        //     .connect()
        //     .await;
        //
        // let mut client = SearcherServiceClient::new(channel);
        // let response = client.send_bundle(BundleRequest {
        //     bundle: Some(Bundle {
        //         packets: bundle_txs.iter().map(|tx| Packet { data: tx.clone(), ..Default::default() }).collect(),
        //         ..Default::default()
        //     }),
        //     ..Default::default()
        // }).await;

        let latency = start.elapsed().as_micros() as u64;

        RelayResult {
            relay_name: "jito".to_string(),
            bundle_id: None, // TODO: extract from response
            success: false,  // TODO: set from actual response
            latency_us: latency,
            error: Some("Not yet implemented".to_string()),
        }
    }

    /// Submit to Nozomi relay.
    async fn submit_to_nozomi(
        url: &str,
        bundle_txs: &[Vec<u8>],
        tip_lamports: u64,
    ) -> RelayResult {
        let start = std::time::Instant::now();

        // TODO: Nozomi-specific submission
        // Nozomi uses a similar gRPC interface to Jito

        let latency = start.elapsed().as_micros() as u64;

        RelayResult {
            relay_name: "nozomi".to_string(),
            bundle_id: None,
            success: false,
            latency_us: latency,
            error: Some("Not yet implemented".to_string()),
        }
    }

    /// Submit to bloXroute.
    async fn submit_to_bloxroute(
        url: &str,
        bundle_txs: &[Vec<u8>],
        tip_lamports: u64,
    ) -> RelayResult {
        let start = std::time::Instant::now();

        // TODO: bloXroute uses REST API for bundle submission
        // POST /api/v2/submit-bundle
        // with base64-encoded transactions

        let latency = start.elapsed().as_micros() as u64;

        RelayResult {
            relay_name: "bloxroute".to_string(),
            bundle_id: None,
            success: false,
            latency_us: latency,
            error: Some("Not yet implemented".to_string()),
        }
    }

    /// Submit to Astralane.
    async fn submit_to_astralane(
        url: &str,
        bundle_txs: &[Vec<u8>],
        tip_lamports: u64,
    ) -> RelayResult {
        let start = std::time::Instant::now();

        // TODO: Astralane Iris routing
        // Astralane aggregates across Jito, Paladin, swQoS

        let latency = start.elapsed().as_micros() as u64;

        RelayResult {
            relay_name: "astralane".to_string(),
            bundle_id: None,
            success: false,
            latency_us: latency,
            error: Some("Not yet implemented".to_string()),
        }
    }

    /// Submit to ZeroSlot.
    async fn submit_to_zeroslot(
        url: &str,
        bundle_txs: &[Vec<u8>],
        tip_lamports: u64,
    ) -> RelayResult {
        let start = std::time::Instant::now();

        // TODO: ZeroSlot submission

        let latency = start.elapsed().as_micros() as u64;

        RelayResult {
            relay_name: "zeroslot".to_string(),
            bundle_id: None,
            success: false,
            latency_us: latency,
            error: Some("Not yet implemented".to_string()),
        }
    }
}
