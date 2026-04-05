use serde_json::json;
use solana_message::AddressLookupTableAccount;
use solana_sdk::{
    hash::Hash,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::Keypair,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use super::RelayResult;
use super::common::{self, RateLimiter};
use crate::config::BotConfig;

/// ZeroSlot uses the same 8 Jito tip accounts.
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

/// ZeroSlot relay — Jito-compatible sendBundle, no auth, uses Jito tip accounts.
pub struct ZeroSlotRelay {
    endpoint: Option<String>,
    http_client: reqwest::Client,
    rate_limiter: RateLimiter,
    tip_index: AtomicUsize,
}

impl ZeroSlotRelay {
    pub fn new(config: &BotConfig) -> Self {
        let endpoint = config.relay_endpoints.zeroslot.clone();
        let tps = common::tps_from_env("ZEROSLOT_TPS", 5.0);
        let min_interval = common::interval_from_tps(tps);
        let http_client = common::build_http_client("zeroslot");

        Self {
            endpoint,
            http_client,
            rate_limiter: RateLimiter::new(min_interval),
            tip_index: AtomicUsize::new(0),
        }
    }

    fn next_tip_account(&self) -> Pubkey {
        let idx = self.tip_index.fetch_add(1, Ordering::Relaxed) % JITO_TIP_ACCOUNTS.len();
        JITO_TIP_ACCOUNTS[idx].parse().unwrap()
    }
}

#[async_trait::async_trait]
impl super::Relay for ZeroSlotRelay {
    fn name(&self) -> &str {
        "zeroslot"
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
            None => return common::fail("zeroslot", "Not configured".to_string()),
        };

        if let Err(r) = self.rate_limiter.check("zeroslot") {
            return r;
        }

        let start = Instant::now();
        let tip_account = self.next_tip_account();

        let encoded = match common::build_signed_bundle_tx(
            "zeroslot", base_instructions, tip_lamports, &tip_account, signer, recent_blockhash, alt,
        ) {
            Ok(enc) => enc,
            Err(mut r) => {
                r.latency_us = start.elapsed().as_micros() as u64;
                return r;
            }
        };

        // Jito-compatible sendBundle
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [[encoded]]
        });

        let result = self.http_client.post(&url).json(&payload).send().await;
        let latency = start.elapsed().as_micros() as u64;

        match result {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(body) => common::parse_jsonrpc_response("zeroslot", &body, latency),
                Err(e) => common::fail_with_latency("zeroslot", crate::config::redact_url(&format!("Parse error: {}", e)), latency),
            },
            Err(e) => common::fail_with_latency("zeroslot", crate::config::redact_url(&format!("Request failed: {}", e)), latency),
        }
    }
}
