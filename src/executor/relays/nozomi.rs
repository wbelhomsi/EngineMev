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

/// Nozomi relay — Jito-compatible sendBundle, no auth, uses centralized Jito tip accounts.
pub struct NozomiRelay {
    endpoint: Option<String>,
    http_client: reqwest::Client,
    rate_limiter: RateLimiter,
}

impl NozomiRelay {
    pub fn new(config: &BotConfig) -> Self {
        let endpoint = config.relay_endpoints.nozomi.clone();
        let tps = common::tps_from_env("NOZOMI_TPS", 5.0);
        let min_interval = common::interval_from_tps(tps);
        let http_client = common::build_http_client("nozomi");

        Self {
            endpoint,
            http_client,
            rate_limiter: RateLimiter::new(min_interval),
        }
    }
}

#[async_trait::async_trait]
impl super::Relay for NozomiRelay {
    fn name(&self) -> &str {
        "nozomi"
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
    ) -> RelayResult {
        let url = match &self.endpoint {
            Some(url) => url.clone(),
            None => {
                let r = common::fail("nozomi", "Not configured".to_string());
                common::record_relay_metrics(&r);
                return r;
            }
        };

        if let Err(r) = self.rate_limiter.check("nozomi") {
            common::record_relay_metrics(&r);
            return r;
        }

        let start = Instant::now();
        let tip_account = common::random_jito_tip_account();

        let serialized = match common::build_signed_bundle_tx(
            "nozomi", base_instructions, tip_lamports, &tip_account, signer, recent_blockhash, alts,
        ) {
            Ok(bytes) => bytes,
            Err(mut r) => {
                r.latency_us = start.elapsed().as_micros() as u64;
                common::record_relay_metrics(&r);
                return r;
            }
        };
        let encoded = common::encode_base58(&serialized);

        // Nozomi uses Jito-compatible sendBundle — base58 encoded
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [[encoded]]
        });

        let result = self.http_client.post(&url).json(&payload).send().await;
        let latency = start.elapsed().as_micros() as u64;

        let result = match result {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(body) => common::parse_jsonrpc_response("nozomi", &body, latency),
                Err(e) => common::fail_with_latency("nozomi", crate::config::redact_url(&format!("Parse error: {}", e)), latency),
            },
            Err(e) => common::fail_with_latency("nozomi", crate::config::redact_url(&format!("Request failed: {}", e)), latency),
        };
        common::record_relay_metrics(&result);
        result
    }
}
