use base64::{engine::general_purpose, Engine as _};
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
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::RelayResult;

/// Rate limiter for relay submission.
pub struct RateLimiter {
    last_submit: Mutex<Instant>,
    min_interval: Duration,
}

impl RateLimiter {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            last_submit: Mutex::new(Instant::now() - Duration::from_secs(60)),
            min_interval,
        }
    }

    /// Returns Ok(()) if enough time has passed, Err(RelayResult) if rate limited.
    pub fn check(&self, relay_name: &str) -> Result<(), RelayResult> {
        let mut last = self.last_submit.lock().unwrap_or_else(|e| e.into_inner());
        if last.elapsed() < self.min_interval {
            return Err(RelayResult {
                relay_name: relay_name.to_string(),
                success: false,
                bundle_id: None,
                error: Some("Rate limited".to_string()),
                latency_us: 0,
            });
        }
        *last = Instant::now();
        Ok(())
    }
}

/// Compute rate limit interval from TPS, adding 10ms padding.
pub fn interval_from_tps(tps: f64) -> Duration {
    if tps > 0.0 {
        Duration::from_millis((1000.0 / tps) as u64 + 10)
    } else {
        Duration::from_millis(1000)
    }
}

/// Read a TPS value from an environment variable with a default.
pub fn tps_from_env(var_name: &str, default: f64) -> f64 {
    std::env::var(var_name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Build a signed, serialized, base64-encoded transaction.
///
/// Appends a tip transfer instruction, tries V0 with ALT, falls back to legacy.
/// Returns Ok(base64_string) or Err(RelayResult).
pub fn build_signed_bundle_tx(
    relay_name: &str,
    base_instructions: &[Instruction],
    tip_lamports: u64,
    tip_account: &Pubkey,
    signer: &Keypair,
    recent_blockhash: Hash,
    alt: Option<&AddressLookupTableAccount>,
) -> Result<String, RelayResult> {
    let mut instructions = base_instructions.to_vec();
    instructions.push(system_instruction::transfer(
        &signer.pubkey(),
        tip_account,
        tip_lamports,
    ));

    // Try V0 with ALT, fall back to legacy
    let tx = if let Some(alt_account) = alt {
        match v0::Message::try_compile(
            &signer.pubkey(),
            &instructions,
            std::slice::from_ref(alt_account),
            recent_blockhash,
        ) {
            Ok(v0_msg) => {
                VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[signer])
                    .map_err(|e| fail(relay_name, format!("V0 sign error: {}", e)))?
            }
            Err(e) => {
                tracing::warn!("[{}] V0 compile failed, falling back to legacy: {}", relay_name, e);
                build_legacy_tx(&instructions, signer, recent_blockhash)?
            }
        }
    } else {
        build_legacy_tx(&instructions, signer, recent_blockhash)?
    };

    // Serialize and size check
    let serialized = bincode::serialize(&tx)
        .map_err(|e| fail(relay_name, format!("Serialize error: {}", e)))?;

    if serialized.len() > 1232 {
        return Err(fail(
            relay_name,
            format!("Tx too large: {} bytes (limit 1232)", serialized.len()),
        ));
    }
    if serialized.len() > 1100 {
        tracing::warn!("[{}] tx near size limit ({} bytes)", relay_name, serialized.len());
    }

    Ok(general_purpose::STANDARD.encode(&serialized))
}

fn build_legacy_tx(
    instructions: &[Instruction],
    signer: &Keypair,
    recent_blockhash: Hash,
) -> Result<VersionedTransaction, RelayResult> {
    let tx = Transaction::new_signed_with_payer(
        instructions,
        Some(&signer.pubkey()),
        &[signer],
        recent_blockhash,
    );
    Ok(VersionedTransaction::from(tx))
}

/// Create a failure RelayResult.
pub fn fail(relay_name: &str, error: String) -> RelayResult {
    RelayResult {
        relay_name: relay_name.to_string(),
        success: false,
        bundle_id: None,
        error: Some(error),
        latency_us: 0,
    }
}

/// Create a failure RelayResult with latency.
pub fn fail_with_latency(relay_name: &str, error: String, latency_us: u64) -> RelayResult {
    RelayResult {
        relay_name: relay_name.to_string(),
        success: false,
        bundle_id: None,
        error: Some(error),
        latency_us,
    }
}

/// Parse a standard JSON-RPC response for bundle submission.
///
/// Looks for `result` (string bundle ID) on success, `error` on failure.
pub fn parse_jsonrpc_response(
    relay_name: &str,
    body: &serde_json::Value,
    latency_us: u64,
) -> RelayResult {
    if let Some(bundle_id) = body.get("result").and_then(|v| v.as_str()) {
        RelayResult {
            relay_name: relay_name.to_string(),
            bundle_id: Some(bundle_id.to_string()),
            success: true,
            latency_us,
            error: None,
        }
    } else {
        let error = body
            .get("error")
            .map(|e| format!("{}", e))
            .unwrap_or_else(|| "Unexpected response format".to_string());
        RelayResult {
            relay_name: relay_name.to_string(),
            bundle_id: None,
            success: false,
            latency_us,
            error: Some(error),
        }
    }
}

/// Build a standard reqwest HTTP client for relay submission.
pub fn build_http_client(relay_name: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .pool_max_idle_per_host(4)
        .pool_idle_timeout(Duration::from_secs(300))
        .tcp_keepalive(Duration::from_secs(30))
        .tcp_nodelay(true)
        .build()
        .unwrap_or_else(|_| panic!("Failed to build {} HTTP client", relay_name))
}
