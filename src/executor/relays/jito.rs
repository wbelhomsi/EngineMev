use serde_json::json;
use solana_sdk::{
    address_lookup_table::AddressLookupTableAccount,
    hash::Hash,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::Keypair,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tracing::debug;

use super::RelayResult;
use super::common::{self, RateLimiter};
use crate::config::BotConfig;

/// Jito tip accounts — bundles must include a SOL transfer to one of these.
const JITO_TIP_ACCOUNTS: &[&str] = &[
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "HFqU5x63VTqvQss8hp11i4bPKELzFLDELBGnNYpzHCDf",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSLzzWw1pa8U5j7cUi2",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
];

/// Jito relay — owns its 8 tip accounts, rate limiter, auth header, JSON-RPC submission.
pub struct JitoRelay {
    endpoint: Option<String>,
    http_client: reqwest::Client,
    rate_limiter: RateLimiter,
    tip_index: AtomicUsize,
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
            tip_index: AtomicUsize::new(0),
            auth_uuid,
        }
    }

    /// Get the next tip account (rotated per bundle).
    fn next_tip_account(&self) -> Pubkey {
        let idx = self.tip_index.fetch_add(1, Ordering::Relaxed) % JITO_TIP_ACCOUNTS.len();
        JITO_TIP_ACCOUNTS[idx].parse().unwrap()
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
        alt: Option<&AddressLookupTableAccount>,
    ) -> RelayResult {
        let url = match &self.endpoint {
            Some(url) => url.clone(),
            None => return common::fail("jito", "Not configured".to_string()),
        };

        if let Err(r) = self.rate_limiter.check("jito") {
            return r;
        }

        let start = Instant::now();
        let tip_account = self.next_tip_account();

        let encoded = match common::build_signed_bundle_tx(
            "jito", base_instructions, tip_lamports, &tip_account, signer, recent_blockhash, alt,
        ) {
            Ok(enc) => enc,
            Err(mut r) => {
                r.latency_us = start.elapsed().as_micros() as u64;
                return r;
            }
        };

        // JSON-RPC payload
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [
                [encoded],
                { "encoding": "base64" }
            ]
        });

        let mut req = self.http_client.post(&url).json(&payload);
        if let Some(ref auth) = self.auth_uuid {
            req = req.header("x-jito-auth", auth);
        }

        let result = req.send().await;
        let latency = start.elapsed().as_micros() as u64;

        match result {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(body) => {
                    debug!("Jito response: {}", body);
                    common::parse_jsonrpc_response("jito", &body, latency)
                }
                Err(e) => common::fail_with_latency("jito", format!("Response parse error: {}", e), latency),
            },
            Err(e) => common::fail_with_latency("jito", format!("Request failed: {}", e), latency),
        }
    }
}
