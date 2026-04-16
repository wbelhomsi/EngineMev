//! RPC helper functions for keypair loading, ALT fetching, transaction simulation,
//! and public transaction submission.

use anyhow::Result;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use tracing::{info, warn};

/// Load the searcher keypair.
/// Tries SEARCHER_PRIVATE_KEY env var (base58) first, then falls back to JSON file.
pub fn load_keypair(path: &str) -> Result<Keypair> {
    // Try base58 private key from env var first
    if let Ok(pk_b58) = std::env::var("SEARCHER_PRIVATE_KEY") {
        let bytes = bs58::decode(pk_b58.trim())
            .into_vec()
            .map_err(|e| anyhow::anyhow!("Invalid base58 SEARCHER_PRIVATE_KEY: {}", e))?;
        let keypair = Keypair::try_from(bytes.as_slice())
            .map_err(|e| anyhow::anyhow!("Invalid keypair bytes: {}", e))?;
        info!("Loaded searcher keypair from SEARCHER_PRIVATE_KEY: {}", keypair.pubkey());
        return Ok(keypair);
    }

    // Fall back to JSON file
    let data = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to read keypair file {}: {}", path, e))?;
    let bytes: Vec<u8> = serde_json::from_str(&data)
        .map_err(|e| anyhow::anyhow!("Invalid keypair JSON in {}: {}", path, e))?;
    let keypair = Keypair::try_from(bytes.as_slice())
        .map_err(|e| anyhow::anyhow!("Invalid keypair bytes in {}: {}", path, e))?;
    info!("Loaded searcher keypair from {}: {}", path, keypair.pubkey());
    Ok(keypair)
}

/// Load an Address Lookup Table from on-chain via getAccountInfo.
/// Returns an AddressLookupTableAccount suitable for v0::Message::try_compile.
pub async fn load_alt(
    client: &reqwest::Client,
    rpc_url: &str,
    alt_address: &str,
) -> Result<solana_message::AddressLookupTableAccount> {
    use base64::{engine::general_purpose, Engine as _};
    use solana_address_lookup_table_interface::state::AddressLookupTable;

    let alt_pubkey: Pubkey = alt_address.parse()
        .map_err(|e| anyhow::anyhow!("Invalid ALT_ADDRESS '{}': {}", alt_address, e))?;

    let payload = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "getAccountInfo",
        "params": [alt_address, {"encoding": "base64"}]
    });

    let resp = client.post(rpc_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send().await?
        .json::<serde_json::Value>().await?;

    let b64 = resp["result"]["value"]["data"][0]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("ALT account {} not found on-chain", alt_address))?;

    let data = general_purpose::STANDARD.decode(b64)?;

    let lookup_table = AddressLookupTable::deserialize(&data)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize ALT: {}", e))?;

    Ok(solana_message::AddressLookupTableAccount {
        key: alt_pubkey,
        addresses: lookup_table.addresses.to_vec(),
    })
}

/// Simulate a V0 transaction via RPC simulateTransaction.
/// Builds a proper versioned transaction matching what relays actually send.
/// Logs the result (success/failure + program logs) for debugging.
pub async fn simulate_v0_tx(
    client: &reqwest::Client,
    rpc_url: &str,
    instructions: &[solana_sdk::instruction::Instruction],
    signer: &solana_sdk::signature::Keypair,
    recent_blockhash: solana_sdk::hash::Hash,
    alts: &[&solana_message::AddressLookupTableAccount],
    context: &str,
) {
    use base64::{engine::general_purpose, Engine as _};
    use solana_sdk::signer::Signer;

    // Build V0 versioned tx (same as relays do)
    let alt_vec: Vec<solana_message::AddressLookupTableAccount> =
        alts.iter().map(|a| (*a).clone()).collect();

    let tx = match solana_sdk::message::v0::Message::try_compile(
        &signer.pubkey(),
        instructions,
        &alt_vec,
        recent_blockhash,
    ) {
        Ok(v0_msg) => {
            match solana_sdk::transaction::VersionedTransaction::try_new(
                solana_sdk::message::VersionedMessage::V0(v0_msg),
                &[signer],
            ) {
                Ok(tx) => tx,
                Err(e) => {
                    warn!("SIM BUILD FAILED [{}]: V0 sign error: {}", context, e);
                    return;
                }
            }
        }
        Err(e) => {
            warn!("SIM BUILD FAILED [{}]: V0 compile error: {}", context, e);
            return;
        }
    };

    let bytes = match bincode::serialize(&tx) {
        Ok(b) => b,
        Err(e) => {
            warn!("SIM BUILD FAILED [{}]: serialize error: {}", context, e);
            return;
        }
    };

    let tx_b64 = general_purpose::STANDARD.encode(&bytes);

    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "simulateTransaction",
        "params": [
            tx_b64,
            {
                "encoding": "base64",
                "sigVerify": false,
                "commitment": "confirmed"
            }
        ]
    });

    match client
        .post(rpc_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) => {
            match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    let result = &json["result"]["value"];
                    let err = &result["err"];
                    let units = result["unitsConsumed"].as_u64().unwrap_or(0);
                    let logs = result["logs"]
                        .as_array()
                        .map(|a| a.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join("\n  "))
                        .unwrap_or_default();

                    if err.is_null() {
                        info!("SIM OK [{}] | CU={} | logs:\n  {}", context, units, logs);
                    } else {
                        warn!("SIM FAIL [{}] | err={} | CU={} | logs:\n  {}", context, err, units, logs);
                        crate::metrics::counters::inc_simulation_errors();
                    }
                }
                Err(e) => {
                    warn!("SIM parse error [{}]: {}", context, e);
                    crate::metrics::counters::inc_simulation_errors();
                }
            }
        }
        Err(e) => {
            warn!("SIM request failed [{}]: {}", context, crate::config::redact_url(&e.to_string()));
            crate::metrics::counters::inc_simulation_errors();
        }
    }
}

/// Send ONE transaction via public RPC (sendTransaction) for on-chain verification.
/// This bypasses Jito bundles entirely — goes through normal tx processing.
/// Costs: tx fee (~5000 lamports) + priority fee. minimum_amount_out protects against loss.
pub async fn send_public_tx(
    client: &reqwest::Client,
    rpc_url: &str,
    base_instructions: &[solana_sdk::instruction::Instruction],
    signer: &solana_sdk::signature::Keypair,
    recent_blockhash: solana_sdk::hash::Hash,
    alts: &[&solana_message::AddressLookupTableAccount],
) {
    use base64::{engine::general_purpose, Engine as _};
    use solana_sdk::message::{v0, VersionedMessage};
    use solana_sdk::transaction::{Transaction, VersionedTransaction};

    // Try V0 with ALTs first, fall back to legacy
    let tx_bytes = if !alts.is_empty() {
        let alt_vec: Vec<solana_message::AddressLookupTableAccount> = alts.iter().map(|a| (*a).clone()).collect();
        match v0::Message::try_compile(
            &signer.pubkey(),
            base_instructions,
            &alt_vec,
            recent_blockhash,
        ) {
            Ok(v0_msg) => {
                match VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[signer]) {
                    Ok(tx) => bincode::serialize(&tx).ok(),
                    Err(e) => { warn!("SEND_PUBLIC: V0 sign error: {}", e); None }
                }
            }
            Err(e) => {
                warn!("SEND_PUBLIC: V0 compile failed, trying legacy: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Fall back to legacy if V0 failed
    let tx_bytes = tx_bytes.unwrap_or_else(|| {
        let tx = Transaction::new_signed_with_payer(
            base_instructions,
            Some(&signer.pubkey()),
            &[signer],
            recent_blockhash,
        );
        bincode::serialize(&tx).unwrap_or_default()
    });

    if tx_bytes.is_empty() {
        warn!("SEND_PUBLIC: serialize error");
        return;
    }

    if tx_bytes.len() > 1232 {
        warn!("SEND_PUBLIC: tx too large ({} bytes), skipping", tx_bytes.len());
        return;
    }

    let tx_b64 = general_purpose::STANDARD.encode(&tx_bytes);
    info!("SEND_PUBLIC: sending tx ({} bytes) to public RPC...", tx_bytes.len());

    let payload = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "sendTransaction",
        "params": [
            tx_b64,
            {
                "encoding": "base64",
                "skipPreflight": true,
                "preflightCommitment": "processed",
                "maxRetries": 3
            }
        ]
    });

    match client.post(rpc_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send().await
    {
        Ok(resp) => {
            match resp.json::<serde_json::Value>().await {
                Ok(json) => {
                    if let Some(sig) = json["result"].as_str() {
                        warn!("SEND_PUBLIC SUCCESS: tx signature = {}", sig);
                        warn!("Check: https://solscan.io/tx/{}", sig);
                    } else if let Some(err) = json.get("error") {
                        warn!("SEND_PUBLIC FAILED: {}", err);
                    } else {
                        warn!("SEND_PUBLIC: unexpected response: {}", json);
                    }
                }
                Err(e) => warn!("SEND_PUBLIC: response parse error: {}", e),
            }
        }
        Err(e) => warn!("SEND_PUBLIC: request failed: {}", crate::config::redact_url(&e.to_string())),
    }
}
