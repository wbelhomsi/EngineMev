use base64::{engine::general_purpose, Engine as _};
use serde_json::json;
use solana_sdk::{
    address_lookup_table::AddressLookupTableAccount,
    hash::Hash,
    instruction::Instruction,
    message::{v0, VersionedMessage},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    system_instruction,
    transaction::{Transaction, VersionedTransaction},
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::debug;

use super::RelayResult;
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
    last_submit: Mutex<Instant>,
    tip_index: AtomicUsize,
    min_interval: Duration,
    auth_uuid: Option<String>,
}

impl JitoRelay {
    pub fn new(config: &BotConfig) -> Self {
        let endpoint = Some(format!(
            "{}/api/v1/bundles",
            config.relay_endpoints.jito.trim_end_matches('/')
        ));

        let tps: f64 = std::env::var("JITO_TPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5.0);
        let min_interval = if tps > 0.0 {
            Duration::from_millis((1000.0 / tps) as u64 + 10)
        } else {
            Duration::from_millis(1000)
        };

        let auth_uuid = std::env::var("JITO_AUTH_UUID").ok().filter(|s| !s.is_empty());

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(Duration::from_secs(300))
            .tcp_keepalive(Duration::from_secs(30))
            .tcp_nodelay(true)
            .build()
            .expect("Failed to build Jito HTTP client");

        Self {
            endpoint,
            http_client,
            last_submit: Mutex::new(Instant::now() - Duration::from_secs(60)),
            tip_index: AtomicUsize::new(0),
            min_interval,
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
            None => return RelayResult {
                relay_name: "jito".to_string(),
                bundle_id: None,
                success: false,
                latency_us: 0,
                error: Some("Not configured".to_string()),
            },
        };

        // Rate limit check
        {
            let mut last = self.last_submit.lock().unwrap_or_else(|e| e.into_inner());
            if last.elapsed() < self.min_interval {
                return RelayResult {
                    relay_name: "jito".to_string(),
                    bundle_id: None,
                    success: false,
                    latency_us: 0,
                    error: Some("Rate limited".to_string()),
                };
            }
            *last = Instant::now();
        }

        let start = Instant::now();

        // Clone base instructions and append tip
        let mut instructions = base_instructions.to_vec();
        let tip_account = self.next_tip_account();
        instructions.push(system_instruction::transfer(
            &signer.pubkey(),
            &tip_account,
            tip_lamports,
        ));

        // Build and sign transaction (V0 with ALT if available, legacy fallback)
        let tx_bytes = if let Some(alt) = alt {
            match v0::Message::try_compile(
                &signer.pubkey(), &instructions, &[alt.clone()], recent_blockhash,
            ) {
                Ok(v0_msg) => {
                    match VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[signer]) {
                        Ok(vtx) => match bincode::serialize(&vtx) {
                            Ok(b) => b,
                            Err(e) => return RelayResult {
                                relay_name: "jito".to_string(),
                                bundle_id: None, success: false,
                                latency_us: start.elapsed().as_micros() as u64,
                                error: Some(format!("V0 serialize error: {}", e)),
                            },
                        },
                        Err(e) => return RelayResult {
                            relay_name: "jito".to_string(),
                            bundle_id: None, success: false,
                            latency_us: start.elapsed().as_micros() as u64,
                            error: Some(format!("V0 sign error: {}", e)),
                        },
                    }
                }
                Err(e) => return RelayResult {
                    relay_name: "jito".to_string(),
                    bundle_id: None, success: false,
                    latency_us: start.elapsed().as_micros() as u64,
                    error: Some(format!("V0 compile error: {}", e)),
                },
            }
        } else {
            let tx = Transaction::new_signed_with_payer(
                &instructions, Some(&signer.pubkey()), &[signer], recent_blockhash,
            );
            match bincode::serialize(&tx) {
                Ok(b) => b,
                Err(e) => return RelayResult {
                    relay_name: "jito".to_string(),
                    bundle_id: None, success: false,
                    latency_us: start.elapsed().as_micros() as u64,
                    error: Some(format!("Serialize error: {}", e)),
                },
            }
        };
        if tx_bytes.len() > 1232 {
            return RelayResult {
                relay_name: "jito".to_string(),
                bundle_id: None, success: false,
                latency_us: start.elapsed().as_micros() as u64,
                error: Some(format!("Tx too large: {} bytes (limit 1232)", tx_bytes.len())),
            };
        }
        if tx_bytes.len() > 1100 {
            tracing::warn!("{}: tx near size limit ({} bytes)", self.name(), tx_bytes.len());
        }
        let encoded = general_purpose::STANDARD.encode(&tx_bytes);

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
            },
            Err(e) => RelayResult {
                relay_name: "jito".to_string(),
                bundle_id: None,
                success: false,
                latency_us: latency,
                error: Some(format!("Request failed: {}", e)),
            },
        }
    }
}
