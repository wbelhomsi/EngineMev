use serde_json::json;
use solana_message::AddressLookupTableAccount;
use solana_sdk::{
    hash::Hash,
    instruction::Instruction,
    signature::Keypair,
};
use std::time::Instant;

use super::RelayResult;
use super::common::{self, RateLimiter};
use crate::config::BotConfig;

/// bloXroute relay — REST format, Authorization header, uses centralized Jito tip accounts.
pub struct BloxrouteRelay {
    endpoint: Option<String>,
    http_client: reqwest::Client,
    rate_limiter: RateLimiter,
    auth_header: String,
}

impl BloxrouteRelay {
    pub fn new(config: &BotConfig) -> Self {
        let endpoint = config.relay_endpoints.bloxroute.clone();
        let tps = common::tps_from_env("BLOXROUTE_TPS", 5.0);
        let min_interval = common::interval_from_tps(tps);
        let auth_header = std::env::var("BLOXROUTE_AUTH_HEADER").unwrap_or_default();
        let http_client = common::build_http_client("bloxroute");

        Self {
            endpoint,
            http_client,
            rate_limiter: RateLimiter::new(min_interval),
            auth_header,
        }
    }
}

#[async_trait::async_trait]
impl super::Relay for BloxrouteRelay {
    fn name(&self) -> &str {
        "bloxroute"
    }

    fn is_configured(&self) -> bool {
        self.endpoint.is_some()
    }

    async fn submit(
        &self,
        base_instructions: &[Instruction],
        tip_lamports: u64,
        signer: &Keypair,
        recent_blockhash: Hash,
        alts: &[&AddressLookupTableAccount],
        nonce: Option<crate::cexdex::NonceInfo>,
    ) -> RelayResult {
        let url = match &self.endpoint {
            Some(url) => url.clone(),
            None => {
                let r = common::fail("bloxroute", "Not configured".to_string());
                common::record_relay_metrics(&r);
                return r;
            }
        };

        if let Err(r) = self.rate_limiter.check("bloxroute") {
            common::record_relay_metrics(&r);
            return r;
        }

        let start = Instant::now();
        let tip_account = common::random_jito_tip_account();

        let serialized = match common::build_signed_bundle_tx(
            "bloxroute", base_instructions, tip_lamports, &tip_account, signer, recent_blockhash, alts, nonce,
        ) {
            Ok(bytes) => bytes,
            Err(mut r) => {
                r.latency_us = start.elapsed().as_micros() as u64;
                common::record_relay_metrics(&r);
                return r;
            }
        };
        let encoded = common::encode_base64(&serialized);

        // bloXroute uses a different REST format (not JSON-RPC), base64 encoded
        let payload = json!({
            "transaction": [encoded],
            "useBundle": true,
        });

        let submit_url = format!("{}/api/v2/submit-bundle", url.trim_end_matches('/'));

        let http_result = self.http_client
            .post(&submit_url)
            .header("Authorization", &self.auth_header)
            .json(&payload)
            .send()
            .await;

        let latency = start.elapsed().as_micros() as u64;

        let result = match http_result {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(body) => {
                    // bloXroute returns bundleId, not result
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
                Err(e) => common::fail_with_latency("bloxroute", crate::config::redact_url(&format!("Parse error: {}", e)), latency),
            },
            Err(e) => common::fail_with_latency("bloxroute", crate::config::redact_url(&format!("Request failed: {}", e)), latency),
        };
        common::record_relay_metrics(&result);
        result
    }
}
