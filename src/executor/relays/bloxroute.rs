use base64::{engine::general_purpose, Engine as _};
use serde_json::json;
use solana_sdk::{
    hash::Hash,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    system_instruction,
    transaction::Transaction,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::RelayResult;
use crate::config::BotConfig;

/// bloXroute uses the same 8 Jito tip accounts.
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

/// bloXroute relay — REST format, Authorization header, uses Jito tip accounts.
pub struct BloxrouteRelay {
    endpoint: Option<String>,
    http_client: reqwest::Client,
    last_submit: Mutex<Instant>,
    tip_index: AtomicUsize,
    min_interval: Duration,
    auth_header: String,
}

impl BloxrouteRelay {
    pub fn new(config: &BotConfig) -> Self {
        let endpoint = config.relay_endpoints.bloxroute.clone();

        let tps: f64 = std::env::var("BLOXROUTE_TPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5.0);
        let min_interval = if tps > 0.0 {
            Duration::from_millis((1000.0 / tps) as u64 + 10)
        } else {
            Duration::from_millis(1000)
        };

        let auth_header = std::env::var("BLOXROUTE_AUTH_HEADER").unwrap_or_default();

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(Duration::from_secs(300))
            .tcp_keepalive(Duration::from_secs(30))
            .tcp_nodelay(true)
            .build()
            .expect("Failed to build bloXroute HTTP client");

        Self {
            endpoint,
            http_client,
            last_submit: Mutex::new(Instant::now() - Duration::from_secs(60)),
            tip_index: AtomicUsize::new(0),
            min_interval,
            auth_header,
        }
    }

    fn next_tip_account(&self) -> Pubkey {
        let idx = self.tip_index.fetch_add(1, Ordering::Relaxed) % JITO_TIP_ACCOUNTS.len();
        JITO_TIP_ACCOUNTS[idx].parse().unwrap()
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
    ) -> RelayResult {
        let url = match &self.endpoint {
            Some(url) => url.clone(),
            None => return RelayResult {
                relay_name: "bloxroute".to_string(),
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
                    relay_name: "bloxroute".to_string(),
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

        // Build and sign transaction
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&signer.pubkey()),
            &[signer],
            recent_blockhash,
        );

        // Serialize and encode
        let tx_bytes = match bincode::serialize(&tx) {
            Ok(b) => b,
            Err(e) => return RelayResult {
                relay_name: "bloxroute".to_string(),
                bundle_id: None,
                success: false,
                latency_us: start.elapsed().as_micros() as u64,
                error: Some(format!("Serialize error: {}", e)),
            },
        };
        let encoded = general_purpose::STANDARD.encode(&tx_bytes);

        // bloXroute uses a different REST format (not JSON-RPC)
        let payload = json!({
            "transaction": [encoded],
            "useBundle": true,
        });

        let submit_url = format!("{}/api/v2/submit-bundle", url.trim_end_matches('/'));

        let result = self.http_client
            .post(&submit_url)
            .header("Authorization", &self.auth_header)
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
}
