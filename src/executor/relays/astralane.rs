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
use tracing::{debug, warn};

use super::RelayResult;
use crate::config::BotConfig;

/// Astralane tip accounts — 17 rotating tip accounts.
const ASTRALANE_TIP_ACCOUNTS: &[&str] = &[
    "astrazznxsGUhWShqgNtAdfrzP2G83DzcWVJDxwV9bF",
    "astra4uejePWneqNaJKuFFA8oonqCE1sqF6b45kDMZm",
    "astra9xWY93QyfG6yM8zwsKsRodscjQ2uU2HKNL5prk",
    "astraRVUuTHjpwEVvNBeQEgwYx9w9CFyfxjYoobCZhL",
    "astraEJ2fEj8Xmy6KLG7B3VfbKfsHXhHrNdCQx7iGJK",
    "astraubkDw81n4LuutzSQ8uzHCv4BhPVhfvTcYv8SKC",
    "astraZW5GLFefxNPAatceHhYjfA1ciq9gvfEg2S47xk",
    "astrawVNP4xDBKT7rAdxrLYiTSTdqtUr63fSMduivXK",
    "AstrA1ejL4UeXC2SBP4cpeEmtcFPZVLxx3XGKXyCW6to",
    "AsTra79FET4aCKWspPqeSFvjJNyp96SvAnrmyAxqg5b7",
    "AstrABAu8CBTyuPXpV4eSCJ5fePEPnxN8NqBaPKQ9fHR",
    "AsTRADtvb6tTmrsqULQ9Wji9PigDMjhfEMza6zkynEvV",
    "AsTRAEoyMofR3vUPpf9k68Gsfb6ymTZttEtsAbv8Bk4d",
    "AStrAJv2RN2hKCHxwUMtqmSxgdcNZbihCwc1mCSnG83W",
    "Astran35aiQUF57XZsmkWMtNCtXGLzs8upfiqXxth2bz",
    "AStRAnpi6kFrKypragExgeRoJ1QnKH7pbSjLAKQVWUum",
    "ASTRaoF93eYt73TYvwtsv6fMWHWbGmMUZfVZPo3CRU9C",
];

/// Astralane relay — owns 17 tip accounts, revert protection, keepalive.
pub struct AstralaneRelay {
    endpoint: Option<String>,
    http_client: reqwest::Client,
    last_submit: Mutex<Instant>,
    tip_index: AtomicUsize,
    min_interval: Duration,
    api_key: String,
}

impl AstralaneRelay {
    pub fn new(config: &BotConfig, shutdown_rx: tokio::sync::watch::Receiver<bool>) -> Self {
        let endpoint = config.relay_endpoints.astralane.clone();

        let tps: f64 = std::env::var("ASTRALANE_TPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(40.0);
        let min_interval = if tps > 0.0 {
            Duration::from_millis((1000.0 / tps) as u64 + 10)
        } else {
            Duration::from_millis(1000)
        };

        let api_key = std::env::var("ASTRALANE_API_KEY").unwrap_or_default();

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(8)
            .pool_idle_timeout(Duration::from_secs(300))
            .tcp_keepalive(Duration::from_secs(30))
            .tcp_nodelay(true)
            .http2_keep_alive_interval(Duration::from_secs(15))
            .http2_keep_alive_timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to build Astralane HTTP client");

        let relay = Self {
            endpoint: endpoint.clone(),
            http_client: http_client.clone(),
            last_submit: Mutex::new(Instant::now() - Duration::from_secs(60)),
            tip_index: AtomicUsize::new(0),
            min_interval,
            api_key: api_key.clone(),
        };

        // Spawn keepalive if configured
        if let Some(url) = endpoint {
            Self::spawn_keepalive(http_client, url, api_key, shutdown_rx);
        }

        relay
    }

    /// Spawn a background keepalive loop.
    /// Pings getHealth every 30s to keep the TCP connection hot.
    fn spawn_keepalive(
        client: reqwest::Client,
        url: String,
        api_key: String,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() { break; }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {
                        let payload = json!({
                            "jsonrpc": "2.0", "id": 1, "method": "getHealth"
                        });
                        let _ = client
                            .post(&url)
                            .header("api_key", &api_key)
                            .json(&payload)
                            .send()
                            .await;
                    }
                }
            }
        });
    }

    /// Get the next tip account (rotated per bundle).
    fn next_tip_account(&self) -> Pubkey {
        let idx = self.tip_index.fetch_add(1, Ordering::Relaxed) % ASTRALANE_TIP_ACCOUNTS.len();
        ASTRALANE_TIP_ACCOUNTS[idx].parse().unwrap()
    }

    /// Build the URL with api-key query param appended.
    fn url_with_auth(&self, base_url: &str) -> String {
        if self.api_key.is_empty() {
            return base_url.to_string();
        }
        if base_url.contains('?') {
            format!("{}&api-key={}", base_url, self.api_key)
        } else {
            format!("{}?api-key={}", base_url, self.api_key)
        }
    }
}

#[async_trait::async_trait]
impl super::Relay for AstralaneRelay {
    fn name(&self) -> &str {
        "astralane"
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
                relay_name: "astralane".to_string(),
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
                    relay_name: "astralane".to_string(),
                    bundle_id: None,
                    success: false,
                    latency_us: 0,
                    error: Some("Rate limited".to_string()),
                };
            }
            *last = Instant::now();
        }

        let start = Instant::now();

        // Clone base instructions and append Astralane tip
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
                                relay_name: "astralane".to_string(),
                                bundle_id: None, success: false,
                                latency_us: start.elapsed().as_micros() as u64,
                                error: Some(format!("V0 serialize error: {}", e)),
                            },
                        },
                        Err(e) => return RelayResult {
                            relay_name: "astralane".to_string(),
                            bundle_id: None, success: false,
                            latency_us: start.elapsed().as_micros() as u64,
                            error: Some(format!("V0 sign error: {}", e)),
                        },
                    }
                }
                Err(e) => return RelayResult {
                    relay_name: "astralane".to_string(),
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
                    relay_name: "astralane".to_string(),
                    bundle_id: None, success: false,
                    latency_us: start.elapsed().as_micros() as u64,
                    error: Some(format!("Serialize error: {}", e)),
                },
            }
        };
        if tx_bytes.len() > 1232 {
            return RelayResult {
                relay_name: "astralane".to_string(),
                bundle_id: None, success: false,
                latency_us: start.elapsed().as_micros() as u64,
                error: Some(format!("Tx too large: {} bytes (limit 1232)", tx_bytes.len())),
            };
        }
        if tx_bytes.len() > 1100 {
            tracing::warn!("{}: tx near size limit ({} bytes)", self.name(), tx_bytes.len());
        }
        let encoded = general_purpose::STANDARD.encode(&tx_bytes);

        // JSON-RPC payload with revertProtection
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendBundle",
            "params": [
                [encoded],
                {
                    "encoding": "base64",
                    "revertProtection": true
                }
            ]
        });

        let url_with_auth = self.url_with_auth(&url);

        let result = self.http_client
            .post(&url_with_auth)
            .header("api_key", &self.api_key)
            .json(&payload)
            .send()
            .await;

        let latency = start.elapsed().as_micros() as u64;

        match result {
            Ok(resp) => {
                let status = resp.status();
                match resp.text().await {
                    Ok(text) => {
                        if !status.is_success() {
                            warn!("Astralane HTTP {}: {}", status, &text[..text.len().min(200)]);
                        }
                        match serde_json::from_str::<serde_json::Value>(&text) {
                            Ok(body) => {
                                debug!("Astralane response: {}", body);
                                let bundle_id = body.get("result")
                                    .and_then(|v| {
                                        if let Some(arr) = v.as_array() {
                                            arr.first().and_then(|s| s.as_str()).map(String::from)
                                        } else if let Some(s) = v.as_str() {
                                            Some(s.to_string())
                                        } else if !v.is_null() {
                                            Some(format!("{}", v))
                                        } else {
                                            None
                                        }
                                    });
                                let success = bundle_id.is_some();
                                let error = if !success {
                                    if let Some(err) = body.get("error") {
                                        Some(format!("{}", err))
                                    } else {
                                        Some(format!("Astralane: {}", body))
                                    }
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
                                error: Some(format!(
                                    "JSON parse error: {} (raw: {})",
                                    e, &text[..text.len().min(200)]
                                )),
                            },
                        }
                    }
                    Err(e) => RelayResult {
                        relay_name: "astralane".to_string(),
                        bundle_id: None,
                        success: false,
                        latency_us: latency,
                        error: Some(format!("Body read error: {}", e)),
                    },
                }
            }
            Err(e) => RelayResult {
                relay_name: "astralane".to_string(),
                bundle_id: None,
                success: false,
                latency_us: latency,
                error: Some(format!("Request failed: {}", e)),
            },
        }
    }
}
