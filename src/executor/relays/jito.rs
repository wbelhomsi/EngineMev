use serde_json::json;
use solana_message::AddressLookupTableAccount;
use solana_sdk::{
    hash::Hash,
    instruction::Instruction,
    signature::Keypair,
};
use std::time::Instant;
use tracing::debug;

use super::RelayResult;
use super::common::{self, RateLimiter};
use crate::config::BotConfig;

/// Jito relay — uses centralized tip accounts from common.rs,
/// rate limiter, auth header, JSON-RPC submission.
pub struct JitoRelay {
    endpoint: Option<String>,
    http_client: reqwest::Client,
    rate_limiter: RateLimiter,
    auth_uuid: Option<String>,
}

impl JitoRelay {
    pub fn new(config: &BotConfig) -> Self {
        let endpoint = Some(format!(
            "{}/api/v1/bundles",
            config.relay_endpoints.jito.trim_end_matches('/')
        ));

        let tps = common::tps_from_env("JITO_TPS", 5.0);
        let min_interval = common::interval_from_tps(tps);
        let auth_uuid = std::env::var("JITO_AUTH_UUID").ok().filter(|s| !s.is_empty());

        let http_client = common::build_http_client("jito");

        Self {
            endpoint,
            http_client,
            rate_limiter: RateLimiter::new(min_interval),
            auth_uuid,
        }
    }
}

#[async_trait::async_trait]
impl super::Relay for JitoRelay {
    fn name(&self) -> &str {
        "jito"
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
                let r = common::fail("jito", "Not configured".to_string());
                common::record_relay_metrics(&r);
                return r;
            }
        };

        if let Err(r) = self.rate_limiter.check("jito") {
            common::record_relay_metrics(&r);
            return r;
        }

        let start = Instant::now();
        let tip_account = common::random_jito_tip_account();

        let serialized = match common::build_signed_bundle_tx(
            "jito", base_instructions, tip_lamports, &tip_account, signer, recent_blockhash, alts, nonce,
        ) {
            Ok(bytes) => bytes,
            Err(mut r) => {
                r.latency_us = start.elapsed().as_micros() as u64;
                common::record_relay_metrics(&r);
                return r;
            }
        };
        let encoded = common::encode_base58(&serialized);

        // JSON-RPC payload — Jito sendBundle expects base58-encoded transactions
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [[encoded]]
        });

        let mut req = self.http_client.post(&url).json(&payload);
        if let Some(ref auth) = self.auth_uuid {
            req = req.header("x-jito-auth", auth);
        }

        let result = req.send().await;
        let latency = start.elapsed().as_micros() as u64;

        let result = match result {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(body) => {
                    debug!("Jito response: {}", body);
                    common::parse_jsonrpc_response("jito", &body, latency)
                }
                Err(e) => common::fail_with_latency("jito", crate::config::redact_url(&format!("Response parse error: {}", e)), latency),
            },
            Err(e) => common::fail_with_latency("jito", crate::config::redact_url(&format!("Request failed: {}", e)), latency),
        };
        common::record_relay_metrics(&result);
        result
    }
}
