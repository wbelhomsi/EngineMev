use base64::{engine::general_purpose, Engine as _};
use solana_message::AddressLookupTableAccount;
use solana_sdk::{
    hash::Hash,
    instruction::Instruction,
    message::{v0, VersionedMessage},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::{Transaction, VersionedTransaction},
};
use solana_system_interface::instruction as system_instruction;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::RelayResult;

// ─── Centralized Jito tip accounts ─────────────────────────────────────────
// Single source of truth — all relays that need Jito tip accounts import from here.
// Jito requires bundles to write-lock at least one of these accounts.
// Verified against getTipAccounts JSON-RPC endpoint.

/// The 8 canonical Jito tip accounts used for bundle auction priority.
pub const JITO_TIP_ACCOUNTS: &[&str] = &[
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "HFqU5x63VTqvQss8hp11i4bPKELzFLDELBGnNYpzHCDf",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSLzzWw1pa8U5j7cUi2",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
];

/// Parse the canonical Jito tip account list into Pubkeys.
pub fn jito_tip_accounts() -> Vec<Pubkey> {
    JITO_TIP_ACCOUNTS
        .iter()
        .map(|s| s.parse().expect("Invalid hardcoded Jito tip account"))
        .collect()
}

/// Pick a random Jito tip account using subsecond nanos for fast rotation.
pub fn random_jito_tip_account() -> Pubkey {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as usize;
    let idx = nanos % JITO_TIP_ACCOUNTS.len();
    JITO_TIP_ACCOUNTS[idx].parse().unwrap()
}

/// Verify that a compiled transaction write-locks the given tip account.
///
/// Jito rejects bundles that don't write-lock at least one tip account.
/// This function checks the compiled message (V0 or Legacy) to confirm
/// the tip account appears in the writable accounts set.
///
/// For V0 messages, accounts can be writable via static keys or ALT lookups.
/// For Legacy messages, writable accounts are determined by the message header.
pub fn verify_tip_write_locked(
    tx: &VersionedTransaction,
    tip_account: &Pubkey,
    alts: &[&AddressLookupTableAccount],
) -> bool {
    match &tx.message {
        VersionedMessage::Legacy(msg) => {
            // In legacy messages:
            // - Writable signed: indices 0..(num_signers - num_readonly_signed)
            // - Writable unsigned: indices num_signers..(total - num_readonly_unsigned)
            let num_signers = msg.header.num_required_signatures as usize;
            let num_readonly_signed = msg.header.num_readonly_signed_accounts as usize;
            let num_readonly_unsigned = msg.header.num_readonly_unsigned_accounts as usize;
            let total = msg.account_keys.len();

            let writable_signed_end = num_signers.saturating_sub(num_readonly_signed);
            let writable_unsigned_end = total.saturating_sub(num_readonly_unsigned);

            for i in 0..writable_signed_end {
                if msg.account_keys[i] == *tip_account {
                    return true;
                }
            }
            for i in num_signers..writable_unsigned_end {
                if msg.account_keys[i] == *tip_account {
                    return true;
                }
            }
            false
        }
        VersionedMessage::V0(msg) => {
            // Check static writable keys (same logic as legacy)
            let num_signers = msg.header.num_required_signatures as usize;
            let num_readonly_signed = msg.header.num_readonly_signed_accounts as usize;
            let num_readonly_unsigned = msg.header.num_readonly_unsigned_accounts as usize;
            let total_static = msg.account_keys.len();

            let writable_signed_end = num_signers.saturating_sub(num_readonly_signed);
            let writable_unsigned_end = total_static.saturating_sub(num_readonly_unsigned);

            for i in 0..writable_signed_end {
                if msg.account_keys[i] == *tip_account {
                    return true;
                }
            }
            for i in num_signers..writable_unsigned_end {
                if msg.account_keys[i] == *tip_account {
                    return true;
                }
            }

            // Check ALT writable lookups — these are accounts resolved from ALTs
            // that the V0 compiler placed in the writable section.
            for table_lookup in &msg.address_table_lookups {
                // Find the matching ALT to resolve the actual addresses
                if let Some(alt) = alts.iter().find(|a| a.key == table_lookup.account_key) {
                    for &idx in &table_lookup.writable_indexes {
                        if let Some(addr) = alt.addresses.get(idx as usize) {
                            if *addr == *tip_account {
                                return true;
                            }
                        }
                    }
                }
            }
            false
        }
        _ => false,
    }
}

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

/// Builds a signed tx for a relay bundle: caller's base instructions +
/// the relay's tip transfer, optionally prefixed by a durable-nonce advance.
///
/// When `nonce` is `Some(info)`:
///   - `advance_nonce_account(&info.account, &info.authority)` is prepended at ix[0]
///   - `recent_blockhash` MUST be the nonce's current cached hash (caller's job)
///   - The single `signer` signature satisfies both the fee-payer and nonce-
///     authority roles because we require `info.authority == signer.pubkey()`
///     by convention for cexdex; this precondition is NOT enforced here.
///
/// When `nonce` is `None`: unchanged from pre-nonce behavior.
///
/// After compile, the builder verifies the tip account is write-locked in
/// the final message. Jito rejects bundles where no tip account is writable;
/// this guard catches any V0/ALT edge case that drops writability.
///
/// Callers choose their own encoding (base58 or base64) via
/// [`encode_base58`] or [`encode_base64`].
pub fn build_signed_bundle_tx(
    relay_name: &str,
    base_instructions: &[Instruction],
    tip_lamports: u64,
    tip_account: &Pubkey,
    signer: &Keypair,
    recent_blockhash: Hash,
    alts: &[&AddressLookupTableAccount],
    nonce: Option<crate::cexdex::NonceInfo>,
) -> Result<Vec<u8>, RelayResult> {
    // Guard: tip must be > 0 to be meaningful for Jito auction
    if tip_lamports == 0 {
        return Err(fail(
            relay_name,
            "Tip lamports is 0 — Jito requires a non-zero tip transfer".to_string(),
        ));
    }

    // Compose the final instruction list:
    //   [optional nonce_advance] + base_instructions + [tip transfer]
    let mut instructions: Vec<Instruction> = Vec::with_capacity(base_instructions.len() + 2);
    if let Some(info) = nonce {
        use solana_system_interface::instruction::advance_nonce_account;
        debug_assert_eq!(
            info.authority,
            signer.pubkey(),
            "nonce authority must match signer for single-signer cexdex bundles",
        );
        instructions.push(advance_nonce_account(&info.account, &info.authority));
    }
    instructions.extend_from_slice(base_instructions);
    instructions.push(system_instruction::transfer(
        &signer.pubkey(),
        tip_account,
        tip_lamports,
    ));

    // Try V0 with ALTs, fall back to legacy
    let tx = if !alts.is_empty() {
        let alt_vec: Vec<AddressLookupTableAccount> = alts.iter().map(|a| (*a).clone()).collect();
        match v0::Message::try_compile(
            &signer.pubkey(),
            &instructions,
            &alt_vec,
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

    // Validate that the tip account is write-locked in the compiled message.
    // Jito rejects bundles where no tip account is writable. This catches any
    // edge case where V0 message compilation or ALT resolution drops writability.
    if !verify_tip_write_locked(&tx, tip_account, alts) {
        tracing::error!(
            "[{}] CRITICAL: tip account {} is NOT write-locked in compiled tx! \
             This would cause Jito rejection. Message type: {}",
            relay_name, tip_account,
            match &tx.message {
                VersionedMessage::V0(_) => "V0",
                VersionedMessage::Legacy(_) => "Legacy",
                _ => "unknown",
            }
        );
        return Err(fail(
            relay_name,
            format!("Tip account {} not write-locked in compiled message", tip_account),
        ));
    }

    // Serialize and size check
    let serialized = bincode::serialize(&tx)
        .map_err(|e| fail(relay_name, format!("Serialize error: {}", e)))?;

    if serialized.len() > 1232 {
        // Log account details for debugging oversized transactions
        tracing::debug!(
            "[{}] Tx {} bytes (limit 1232), {} instructions, {} accounts in message",
            relay_name, serialized.len(), instructions.len(),
            match &tx.message {
                VersionedMessage::V0(m) => format!("{} static + ALT", m.account_keys.len()),
                VersionedMessage::Legacy(m) => format!("{} legacy", m.account_keys.len()),
                _ => "unknown".to_string(),
            }
        );
        return Err(fail(
            relay_name,
            format!("Tx too large: {} bytes (limit 1232)", serialized.len()),
        ));
    }
    if serialized.len() > 1100 {
        tracing::warn!("[{}] tx near size limit ({} bytes)", relay_name, serialized.len());
    }

    Ok(serialized)
}

/// Encode serialized transaction bytes as base58 (Jito/Nozomi/ZeroSlot default).
///
/// Uses `into_vec()` + unsafe `from_utf8_unchecked` to avoid the UTF-8
/// validation that `into_string()` performs. Base58 output is always valid
/// ASCII, so this is safe and avoids an O(n) scan on every bundle.
pub fn encode_base58(serialized: &[u8]) -> String {
    let bytes = bs58::encode(serialized).into_vec();
    // SAFETY: base58 alphabet is a strict subset of ASCII, always valid UTF-8.
    unsafe { String::from_utf8_unchecked(bytes) }
}

/// Encode serialized transaction bytes as base64.
pub fn encode_base64(serialized: &[u8]) -> String {
    general_purpose::STANDARD.encode(serialized)
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

/// Record metrics for a relay submission result.
pub fn record_relay_metrics(result: &super::RelayResult) {
    let status = if result.success {
        "accepted"
    } else if result.error.as_deref() == Some("Rate limited") {
        "rate_limited"
    } else {
        "rejected"
    };
    crate::metrics::counters::inc_relay_submission(&result.relay_name, status);
    if result.latency_us > 0 {
        crate::metrics::counters::record_relay_latency_us(&result.relay_name, result.latency_us);
    }
    // Categorize errors for deeper debugging
    if !result.success {
        if let Some(ref err) = result.error {
            let error_type = if err.contains("Rate limited") {
                "rate_limited"
            } else if err.contains("Tx too large") {
                "tx_too_large"
            } else if err.contains("sign error") || err.contains("compile failed") {
                "build_error"
            } else if err.contains("Request failed") || err.contains("timeout") {
                "network_error"
            } else if err.contains("already processed") || err.contains("blockhash") {
                "stale_blockhash"
            } else if err.contains("not write-locked") || err.contains("Tip lamports is 0") {
                "tip_error"
            } else {
                "on_chain_error"
            };
            crate::metrics::counters::inc_relay_errors(&result.relay_name, error_type);
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

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::signature::Keypair;
    use solana_sdk::signer::Signer;

    #[test]
    fn test_jito_tip_accounts_parse() {
        let accounts = jito_tip_accounts();
        assert_eq!(accounts.len(), 8);
        // All should be valid pubkeys
        for account in &accounts {
            assert_ne!(*account, Pubkey::default());
        }
    }

    #[test]
    fn test_random_jito_tip_account_returns_valid() {
        let tip = random_jito_tip_account();
        let all = jito_tip_accounts();
        assert!(all.contains(&tip), "Random tip account must be from the canonical list");
    }

    #[test]
    fn test_build_signed_bundle_tx_includes_tip_writable() {
        let signer = Keypair::new();
        let tip_account = random_jito_tip_account();
        let recent_blockhash = Hash::new_unique();

        // Minimal instruction set: just compute budget
        let compute_budget = Pubkey::from_str_const("ComputeBudget111111111111111111111111111111");
        let mut cu_limit_data = vec![2u8];
        cu_limit_data.extend_from_slice(&200_000u32.to_le_bytes());
        let base_ix = Instruction {
            program_id: compute_budget,
            accounts: vec![],
            data: cu_limit_data,
        };

        let result = build_signed_bundle_tx(
            "test", &[base_ix], 50_000, &tip_account, &signer, recent_blockhash, &[], None,
        );
        assert!(result.is_ok(), "Should build successfully: {:?}", result.err());

        // Deserialize and verify tip is writable
        let bytes = result.unwrap();
        let tx: VersionedTransaction = bincode::deserialize(&bytes).unwrap();
        assert!(
            verify_tip_write_locked(&tx, &tip_account, &[]),
            "Tip account must be write-locked in the compiled transaction"
        );
    }

    #[test]
    fn test_build_signed_bundle_tx_rejects_zero_tip() {
        let signer = Keypair::new();
        let tip_account = random_jito_tip_account();
        let recent_blockhash = Hash::new_unique();

        let result = build_signed_bundle_tx(
            "test", &[], 0, &tip_account, &signer, recent_blockhash, &[], None,
        );
        assert!(result.is_err(), "Should reject zero tip");
        let err = result.unwrap_err();
        assert!(err.error.unwrap().contains("Tip lamports is 0"));
    }

    #[test]
    fn test_verify_tip_write_locked_legacy() {
        let signer = Keypair::new();
        let tip_account: Pubkey = "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5".parse().unwrap();
        let recent_blockhash = Hash::new_unique();

        // Build a legacy tx with tip transfer
        let transfer_ix = system_instruction::transfer(
            &signer.pubkey(),
            &tip_account,
            50_000,
        );
        let tx = Transaction::new_signed_with_payer(
            &[transfer_ix],
            Some(&signer.pubkey()),
            &[&signer],
            recent_blockhash,
        );
        let versioned = VersionedTransaction::from(tx);

        assert!(
            verify_tip_write_locked(&versioned, &tip_account, &[]),
            "Tip account should be writable in legacy tx"
        );
    }

    #[test]
    fn test_verify_tip_write_locked_rejects_readonly() {
        use solana_sdk::instruction::AccountMeta;

        let signer = Keypair::new();
        let tip_account: Pubkey = "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5".parse().unwrap();
        let recent_blockhash = Hash::new_unique();

        // Build a tx where tip_account is ONLY referenced as readonly
        let fake_ix = Instruction {
            program_id: Pubkey::new_unique(),
            accounts: vec![
                AccountMeta::new_readonly(tip_account, false), // readonly!
            ],
            data: vec![0],
        };
        let tx = Transaction::new_signed_with_payer(
            &[fake_ix],
            Some(&signer.pubkey()),
            &[&signer],
            recent_blockhash,
        );
        let versioned = VersionedTransaction::from(tx);

        assert!(
            !verify_tip_write_locked(&versioned, &tip_account, &[]),
            "Tip account should NOT be writable when only referenced as readonly"
        );
    }

    #[test]
    fn test_verify_tip_write_locked_v0_static_keys() {
        let signer = Keypair::new();
        let tip_account: Pubkey = "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5".parse().unwrap();
        let recent_blockhash = Hash::new_unique();

        // Build a V0 tx with no ALTs — tip should be in static writable keys
        let transfer_ix = system_instruction::transfer(
            &signer.pubkey(),
            &tip_account,
            50_000,
        );
        let v0_msg = v0::Message::try_compile(
            &signer.pubkey(),
            &[transfer_ix],
            &[], // no ALTs
            recent_blockhash,
        ).unwrap();
        let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[&signer]).unwrap();

        assert!(
            verify_tip_write_locked(&tx, &tip_account, &[]),
            "Tip account should be writable in V0 static keys"
        );
    }

    #[test]
    fn test_verify_tip_write_locked_v0_via_alt() {
        let signer = Keypair::new();
        let tip_account: Pubkey = "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5".parse().unwrap();
        let recent_blockhash = Hash::new_unique();

        // Create an ALT that contains the tip account
        let alt_key = Pubkey::new_unique();
        let alt = AddressLookupTableAccount {
            key: alt_key,
            addresses: vec![tip_account, Pubkey::new_unique()],
        };

        // Build a V0 tx with ALT
        let transfer_ix = system_instruction::transfer(
            &signer.pubkey(),
            &tip_account,
            50_000,
        );
        let v0_msg = v0::Message::try_compile(
            &signer.pubkey(),
            &[transfer_ix],
            &[alt.clone()],
            recent_blockhash,
        ).unwrap();
        let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[&signer]).unwrap();

        assert!(
            verify_tip_write_locked(&tx, &tip_account, &[&alt]),
            "Tip account should be writable via ALT lookup in V0 tx"
        );
    }

    #[test]
    fn test_interval_from_tps() {
        let interval = interval_from_tps(5.0);
        assert_eq!(interval, Duration::from_millis(210)); // 200 + 10 padding

        let interval_zero = interval_from_tps(0.0);
        assert_eq!(interval_zero, Duration::from_millis(1000));
    }

    #[test]
    fn test_rate_limiter_allows_first_call() {
        let limiter = RateLimiter::new(Duration::from_secs(1));
        assert!(limiter.check("test").is_ok());
    }

    #[test]
    fn test_rate_limiter_blocks_rapid_calls() {
        let limiter = RateLimiter::new(Duration::from_secs(10));
        assert!(limiter.check("test").is_ok());
        assert!(limiter.check("test").is_err());
    }

    #[test]
    fn builder_prepends_advance_nonce_when_nonce_info_given() {
        use crate::cexdex::NonceInfo;

        let signer = Keypair::new();
        let tip_account = Pubkey::new_unique();
        let nonce_account = Pubkey::new_unique();
        let nonce_hash = Hash::new_unique();
        let base_ix = system_instruction::transfer(&signer.pubkey(), &Pubkey::new_unique(), 100);

        let tx_bytes = build_signed_bundle_tx(
            "test",
            &[base_ix],
            50_000,
            &tip_account,
            &signer,
            nonce_hash,
            &[],
            Some(NonceInfo {
                account: nonce_account,
                authority: signer.pubkey(),
            }),
        )
        .expect("tx should build");

        // Decode the signed versioned tx
        let tx: solana_sdk::transaction::VersionedTransaction =
            bincode::deserialize(&tx_bytes).expect("decode");

        // ix[0] must be AdvanceNonceAccount (System Program discriminator 4)
        let (ix0_data, actual_bh) = match &tx.message {
            solana_sdk::message::VersionedMessage::Legacy(m) => {
                (m.instructions[0].data.clone(), m.recent_blockhash)
            }
            solana_sdk::message::VersionedMessage::V0(m) => {
                (m.instructions[0].data.clone(), m.recent_blockhash)
            }
            _ => panic!("unexpected message version"),
        };
        let disc = u32::from_le_bytes(ix0_data[0..4].try_into().unwrap());
        assert_eq!(disc, 4, "ix[0] must be System::AdvanceNonceAccount");
        assert_eq!(actual_bh, nonce_hash, "recent_blockhash must be the nonce hash");
    }

    #[test]
    fn builder_unchanged_when_nonce_info_none() {
        let signer = Keypair::new();
        let tip_account = Pubkey::new_unique();
        let hash = Hash::new_unique();
        let base_ix = system_instruction::transfer(&signer.pubkey(), &Pubkey::new_unique(), 100);

        let tx_bytes = build_signed_bundle_tx(
            "test", &[base_ix], 50_000, &tip_account, &signer, hash, &[], None,
        )
        .expect("tx should build");

        let tx: solana_sdk::transaction::VersionedTransaction =
            bincode::deserialize(&tx_bytes).expect("decode");
        let ix0_data = match &tx.message {
            solana_sdk::message::VersionedMessage::Legacy(m) => m.instructions[0].data.clone(),
            solana_sdk::message::VersionedMessage::V0(m) => m.instructions[0].data.clone(),
            _ => panic!("unexpected message version"),
        };
        let disc = u32::from_le_bytes(ix0_data[0..4].try_into().unwrap());
        assert_eq!(disc, 2, "ix[0] must be System::Transfer (the base_ix we passed)");
    }
}
