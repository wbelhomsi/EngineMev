//! Bundle confirmation tracker.
//!
//! After relay dispatch, collects bundle IDs from relay results and polls
//! Jito's `getBundleStatuses` to determine whether bundles actually landed on-chain.
//!
//! Only when a bundle is confirmed do we increment the "confirmed" profit/tip metrics.
//! Bundles that don't confirm within the timeout are counted as "dropped".

use serde_json::json;
use std::time::Duration;
use tracing::{debug, info};

use crate::config;
use crate::metrics::counters;

/// Maximum time to wait for confirmation (~30 slots at 400ms/slot = 12s).
const CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(12);

/// Base polling interval for bundle status checks.
const POLL_INTERVAL: Duration = Duration::from_millis(3000);

/// Maximum number of RPC errors before giving up (avoids hammering rate-limited endpoint).
const MAX_RPC_ERRORS: u32 = 2;

/// Spawn a background task that tracks whether a bundle landed on-chain.
///
/// Collects bundle IDs from the relay result channel, then polls
/// `getBundleStatuses` (Jito) until one confirms or the timeout expires.
///
/// This function is non-blocking -- it spawns a tokio task and returns immediately.
pub fn spawn_confirmation_tracker(
    http_client: reqwest::Client,
    jito_url: String,
    estimated_profit_lamports: u64,
    tip_lamports: u64,
    mut relay_rx: tokio::sync::mpsc::Receiver<crate::executor::relays::RelayResult>,
) {
    tokio::spawn(async move {
        // Phase 1: Collect bundle IDs from relay results (with short timeout).
        // Relays typically respond within 1-5 seconds.
        let mut bundle_ids: Vec<String> = Vec::new();
        let collect_deadline = tokio::time::Instant::now() + Duration::from_secs(6);

        loop {
            let remaining = collect_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, relay_rx.recv()).await {
                Ok(Some(result)) => {
                    if result.success {
                        if let Some(id) = result.bundle_id {
                            if !id.is_empty() {
                                bundle_ids.push(id);
                            }
                        }
                    }
                }
                Ok(None) => break, // Channel closed, all relays responded
                Err(_) => break,   // Timeout
            }
        }

        if bundle_ids.is_empty() {
            debug!("No accepted bundle IDs to track -- all relays rejected or failed");
            return;
        }

        debug!(
            "Tracking {} bundle ID(s) for confirmation (profit={}, tip={})",
            bundle_ids.len(),
            estimated_profit_lamports,
            tip_lamports
        );

        // Phase 2: Poll getBundleStatuses until confirmed or timeout.
        // Jito rate-limits this endpoint to 1 req/sec with 120s backoff,
        // so we use a 3s base interval and bail after 2 consecutive RPC errors.
        let deadline = tokio::time::Instant::now() + CONFIRMATION_TIMEOUT;
        let mut rpc_errors: u32 = 0;

        // Stagger initial poll to avoid thundering herd when many bundles
        // are submitted simultaneously.
        let jitter_ms = {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos();
            (nanos % 2000) as u64
        };
        tokio::time::sleep(Duration::from_millis(1000 + jitter_ms)).await;

        loop {
            if tokio::time::Instant::now() >= deadline {
                info!(
                    "Bundle DROPPED: {} ID(s) not confirmed after {:?}",
                    bundle_ids.len(),
                    CONFIRMATION_TIMEOUT
                );
                counters::inc_bundles_dropped();
                return;
            }

            match check_bundle_statuses(&http_client, &jito_url, &bundle_ids).await {
                ConfirmationStatus::Landed => {
                    info!(
                        "Bundle CONFIRMED on-chain: profit={} tip={} lamports",
                        estimated_profit_lamports, tip_lamports
                    );
                    counters::inc_bundles_confirmed();
                    counters::add_confirmed_profit_lamports(estimated_profit_lamports);
                    counters::add_confirmed_tips_paid_lamports(tip_lamports);
                    return;
                }
                ConfirmationStatus::Failed => {
                    info!("Bundle tx landed but FAILED on-chain");
                    counters::inc_bundles_dropped();
                    return;
                }
                ConfirmationStatus::Pending => {
                    rpc_errors = 0; // Reset on successful response
                }
                ConfirmationStatus::RpcError => {
                    rpc_errors += 1;
                    if rpc_errors >= MAX_RPC_ERRORS {
                        debug!(
                            "Giving up on confirmation after {} RPC errors (rate limited)",
                            rpc_errors
                        );
                        counters::inc_bundles_dropped();
                        return;
                    }
                    // Backoff: double the wait on RPC error
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

#[derive(Debug, PartialEq)]
enum ConfirmationStatus {
    /// Bundle landed and executed successfully.
    Landed,
    /// Bundle landed but the transaction failed.
    Failed,
    /// Bundle not yet seen or still processing.
    Pending,
    /// RPC/network error polling status.
    RpcError,
}

/// Call Jito's `getBundleStatuses` for the given bundle IDs.
///
/// Jito returns statuses with `confirmation_status` of "processed", "confirmed",
/// or "finalized", plus an `err` map.
async fn check_bundle_statuses(
    client: &reqwest::Client,
    jito_url: &str,
    bundle_ids: &[String],
) -> ConfirmationStatus {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getBundleStatuses",
        "params": [bundle_ids]
    });

    let resp = match client.post(jito_url).json(&payload).send().await {
        Ok(r) => r,
        Err(e) => {
            debug!("getBundleStatuses request failed: {}", config::redact_url(&e.to_string()));
            return ConfirmationStatus::RpcError;
        }
    };

    let body: serde_json::Value = match resp.json().await {
        Ok(b) => b,
        Err(e) => {
            debug!("getBundleStatuses parse failed: {}", e);
            return ConfirmationStatus::RpcError;
        }
    };

    // Jito getBundleStatuses returns:
    // { "result": { "context": {...}, "value": [ { "bundle_id": "...", "transactions": [...],
    //   "slot": N, "confirmation_status": "confirmed", "err": {"Ok": null} }, ... ] } }
    let statuses = match body.get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_array())
    {
        Some(arr) => arr,
        None => {
            debug!("getBundleStatuses unexpected response: {}", body);
            return ConfirmationStatus::RpcError;
        }
    };

    for status in statuses {
        // Check confirmation status
        if let Some(confirmation) = status.get("confirmation_status").and_then(|c| c.as_str()) {
            // Check for error
            let has_error = status.get("err")
                .map(|e| {
                    // err is either {"Ok": null} (success) or an error object
                    if let Some(obj) = e.as_object() {
                        !obj.contains_key("Ok")
                    } else {
                        !e.is_null()
                    }
                })
                .unwrap_or(false);

            if has_error {
                debug!("Bundle confirmed with error: {:?}", status.get("err"));
                return ConfirmationStatus::Failed;
            }

            match confirmation {
                "processed" | "confirmed" | "finalized" => {
                    return ConfirmationStatus::Landed;
                }
                _ => {
                    debug!("Unknown bundle confirmation status: {}", confirmation);
                }
            }
        }
    }

    ConfirmationStatus::Pending
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_confirmation_timeout_is_reasonable() {
        // 30 slots at 400ms = 12s
        assert_eq!(CONFIRMATION_TIMEOUT, Duration::from_secs(12));
        assert_eq!(POLL_INTERVAL, Duration::from_millis(3000));
        assert_eq!(MAX_RPC_ERRORS, 2);
        // Should poll ~4 times before timeout (12s / 3s)
        assert!(CONFIRMATION_TIMEOUT.as_millis() / POLL_INTERVAL.as_millis() >= 3);
    }

    #[tokio::test]
    async fn test_check_bundle_landed() {
        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{
                "jsonrpc": "2.0",
                "result": {
                    "context": { "slot": 123456 },
                    "value": [
                        {
                            "bundle_id": "abc123",
                            "transactions": ["sig1"],
                            "slot": 123450,
                            "confirmation_status": "confirmed",
                            "err": { "Ok": null }
                        }
                    ]
                },
                "id": 1
            }"#)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let ids = vec!["abc123".to_string()];
        let status = check_bundle_statuses(&client, &server.url(), &ids).await;
        assert_eq!(status, ConfirmationStatus::Landed);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_check_bundle_pending() {
        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{
                "jsonrpc": "2.0",
                "result": {
                    "context": { "slot": 123456 },
                    "value": []
                },
                "id": 1
            }"#)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let ids = vec!["abc123".to_string()];
        let status = check_bundle_statuses(&client, &server.url(), &ids).await;
        assert_eq!(status, ConfirmationStatus::Pending);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_check_bundle_failed() {
        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{
                "jsonrpc": "2.0",
                "result": {
                    "context": { "slot": 123456 },
                    "value": [
                        {
                            "bundle_id": "abc123",
                            "transactions": ["sig1"],
                            "slot": 123450,
                            "confirmation_status": "confirmed",
                            "err": { "InstructionError": [0, "Custom"] }
                        }
                    ]
                },
                "id": 1
            }"#)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let ids = vec!["abc123".to_string()];
        let status = check_bundle_statuses(&client, &server.url(), &ids).await;
        assert_eq!(status, ConfirmationStatus::Failed);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_check_bundle_rpc_error() {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        let ids = vec!["abc123".to_string()];
        // Use an unreachable URL
        let status = check_bundle_statuses(&client, "http://127.0.0.1:1", &ids).await;
        assert_eq!(status, ConfirmationStatus::RpcError);
    }

    #[tokio::test]
    async fn test_tracker_no_bundle_ids_exits_early() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        // Drop sender immediately
        drop(tx);

        let client = reqwest::Client::new();
        // spawn_confirmation_tracker spawns internally, so we just need to verify it doesn't panic
        spawn_confirmation_tracker(
            client,
            "http://localhost:0".to_string(),
            100_000,
            15_000,
            rx,
        );
        // Give the spawned task a moment to complete
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    #[tokio::test]
    async fn test_check_bundle_finalized() {
        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{
                "jsonrpc": "2.0",
                "result": {
                    "context": { "slot": 123456 },
                    "value": [
                        {
                            "bundle_id": "def456",
                            "transactions": ["sig1", "sig2"],
                            "slot": 123450,
                            "confirmation_status": "finalized",
                            "err": { "Ok": null }
                        }
                    ]
                },
                "id": 1
            }"#)
            .create_async()
            .await;

        let client = reqwest::Client::new();
        let ids = vec!["def456".to_string()];
        let status = check_bundle_statuses(&client, &server.url(), &ids).await;
        assert_eq!(status, ConfirmationStatus::Landed);
        mock.assert_async().await;
    }
}
