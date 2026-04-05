//! CLI tool to create or extend an Address Lookup Table (ALT) with 17 common addresses.
//!
//! Usage:
//!   cargo run --bin setup-alt
//!
//! Environment variables:
//!   RPC_URL              - Solana RPC endpoint (required)
//!   SEARCHER_PRIVATE_KEY - Base58-encoded private key (preferred)
//!   SEARCHER_KEYPAIR     - Path to JSON keypair file (fallback)
//!   ALT_ADDRESS          - Existing ALT to extend (optional; creates new if unset)

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use solana_address_lookup_table_interface::instruction as alt_ix;
use solana_sdk::{
    hash::Hash,
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::Transaction,
};
use std::str::FromStr;

/// The 17 addresses to store in the ALT.
fn alt_addresses() -> Vec<Pubkey> {
    let mut addrs: Vec<Pubkey> = [
        // System programs
        "11111111111111111111111111111111",              // System Program
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",  // SPL Token
        "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",  // Token-2022
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",  // ATA Program
        "ComputeBudget111111111111111111111111111111",    // Compute Budget
        "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",  // Memo Program
        // DEX programs
        "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",  // Raydium AMM
        "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C",  // Raydium CP
        "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK",  // Raydium CLMM
        "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",  // Orca Whirlpool
        "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",  // Meteora DLMM
        "cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG",  // Meteora DAMM v2
        "5ocnV1qiCgaQR8Jb8xWnVbApfaygJ8tNoZfgPwsgx9kx",  // Sanctum S Controller
        "PhoeNiXZ8ByJGLkxNfZRnkUfjvmuYqLR89jjFHGqdXY",  // Phoenix V1
        "MNFSTqtC93rEfYHB6hF82sKdZpUDFWkViLByLd1k1Ms",  // Manifest
        // Token mints
        "So11111111111111111111111111111111111111112",    // wSOL
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", // USDC
        // Arb-guard program
        "CbjPG5TEEhZGXsA8prmJPfvgH51rudYgcubRUtCCGyUw",  // arb-guard program ID
    ]
    .iter()
    .map(|s| Pubkey::from_str(s).expect("hardcoded pubkey must parse"))
    .collect::<Vec<_>>();

    // Add arb-guard PDA (derived at runtime)
    let guard_program = Pubkey::from_str("CbjPG5TEEhZGXsA8prmJPfvgH51rudYgcubRUtCCGyUw").unwrap();
    let searcher = Pubkey::from_str("149xtHKerf2MgJVQ2CZB34bUALs8GaZjZWmQnC9si9yh").unwrap();
    let (guard_pda, _) = Pubkey::find_program_address(
        &[b"guard", searcher.as_ref()],
        &guard_program,
    );
    addrs.push(guard_pda);

    // Add signer's wSOL ATA
    let wsol = Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap();
    let token_program = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
    let (wsol_ata, _) = Pubkey::find_program_address(
        &[searcher.as_ref(), token_program.as_ref(), wsol.as_ref()],
        &Pubkey::from_str("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL").unwrap(),
    );
    addrs.push(wsol_ata);

    addrs
}

/// Load keypair from SEARCHER_PRIVATE_KEY (base58) or SEARCHER_KEYPAIR file (JSON).
fn load_keypair() -> Result<Keypair> {
    // Try base58 private key from env var first
    if let Ok(pk_b58) = std::env::var("SEARCHER_PRIVATE_KEY") {
        let bytes = bs58::decode(pk_b58.trim())
            .into_vec()
            .map_err(|e| anyhow!("Invalid base58 SEARCHER_PRIVATE_KEY: {}", e))?;
        let keypair = Keypair::try_from(bytes.as_slice())
            .map_err(|e| anyhow!("Invalid keypair bytes: {}", e))?;
        eprintln!("Loaded keypair from SEARCHER_PRIVATE_KEY: {}", keypair.pubkey());
        return Ok(keypair);
    }

    // Fall back to JSON file
    let path = std::env::var("SEARCHER_KEYPAIR")
        .map_err(|_| anyhow!("Neither SEARCHER_PRIVATE_KEY nor SEARCHER_KEYPAIR is set"))?;
    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read keypair file {}", path))?;
    let bytes: Vec<u8> = serde_json::from_str(&data)
        .with_context(|| format!("Invalid keypair JSON in {}", path))?;
    let keypair = Keypair::try_from(bytes.as_slice())
        .map_err(|e| anyhow!("Invalid keypair bytes in {}: {}", path, e))?;
    eprintln!("Loaded keypair from {}: {}", path, keypair.pubkey());
    Ok(keypair)
}

/// JSON-RPC helper: send a request and return the "result" field.
fn rpc_call(
    client: &reqwest::blocking::Client,
    rpc_url: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let resp: serde_json::Value = client
        .post(rpc_url)
        .json(&body)
        .send()
        .with_context(|| format!("RPC {} request failed", method))?
        .json()
        .with_context(|| format!("RPC {} response not JSON", method))?;

    if let Some(err) = resp.get("error") {
        return Err(anyhow!("RPC {} error: {}", method, err));
    }
    resp.get("result")
        .cloned()
        .ok_or_else(|| anyhow!("RPC {} response missing 'result'", method))
}

/// Get the latest blockhash from RPC.
fn get_latest_blockhash(
    client: &reqwest::blocking::Client,
    rpc_url: &str,
) -> Result<Hash> {
    let result = rpc_call(
        client,
        rpc_url,
        "getLatestBlockhash",
        serde_json::json!([{"commitment": "finalized"}]),
    )?;
    let hash_str = result["value"]["blockhash"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing blockhash in response"))?;
    Hash::from_str(hash_str).map_err(|e| anyhow!("Invalid blockhash: {}", e))
}

/// Get the current slot from RPC.
fn get_slot(
    client: &reqwest::blocking::Client,
    rpc_url: &str,
) -> Result<u64> {
    let result = rpc_call(
        client,
        rpc_url,
        "getSlot",
        serde_json::json!([{"commitment": "finalized"}]),
    )?;
    result.as_u64().ok_or_else(|| anyhow!("getSlot did not return a number"))
}

/// Send a signed transaction via sendTransaction RPC.
fn send_transaction(
    client: &reqwest::blocking::Client,
    rpc_url: &str,
    tx: &Transaction,
) -> Result<String> {
    let tx_bytes = bincode::serialize(tx)
        .map_err(|e| anyhow!("Failed to serialize transaction: {}", e))?;
    let tx_b64 = general_purpose::STANDARD.encode(&tx_bytes);

    let result = rpc_call(
        client,
        rpc_url,
        "sendTransaction",
        serde_json::json!([
            tx_b64,
            {
                "encoding": "base64",
                "skipPreflight": false,
                "preflightCommitment": "confirmed",
            }
        ]),
    )?;
    result
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("sendTransaction did not return a signature string"))
}

/// Fetch an existing ALT and return the list of addresses it contains.
fn fetch_alt_addresses(
    client: &reqwest::blocking::Client,
    rpc_url: &str,
    alt_address: &Pubkey,
) -> Result<Vec<Pubkey>> {
    let result = rpc_call(
        client,
        rpc_url,
        "getAccountInfo",
        serde_json::json!([
            alt_address.to_string(),
            {"encoding": "base64", "commitment": "confirmed"}
        ]),
    )?;

    let value = result
        .get("value")
        .ok_or_else(|| anyhow!("ALT account {} not found", alt_address))?;
    if value.is_null() {
        return Err(anyhow!("ALT account {} does not exist", alt_address));
    }

    let data_arr = value["data"]
        .as_array()
        .ok_or_else(|| anyhow!("Missing data field in ALT account"))?;
    let data_b64 = data_arr[0]
        .as_str()
        .ok_or_else(|| anyhow!("Missing base64 data in ALT account"))?;
    let data = general_purpose::STANDARD
        .decode(data_b64)
        .map_err(|e| anyhow!("Failed to decode ALT base64: {}", e))?;

    // ALT account layout:
    //   4 bytes: type discriminator (u32 LE = 1 for lookup table)
    //   8 bytes: deactivation slot (u64 LE)
    //   8 bytes: last extended slot (u64 LE)
    //   1 byte:  last extended slot start index
    //   1 byte:  has authority (bool)
    //  32 bytes: authority pubkey (if has_authority)
    //   2 bytes: padding
    // Then: N * 32 bytes of addresses
    //
    // The fixed header with authority is 56 bytes total.
    const HEADER_SIZE: usize = 56;
    if data.len() < HEADER_SIZE {
        return Err(anyhow!(
            "ALT account data too small: {} bytes (expected >= {})",
            data.len(),
            HEADER_SIZE
        ));
    }

    let address_data = &data[HEADER_SIZE..];
    if address_data.len() % 32 != 0 {
        return Err(anyhow!(
            "ALT address data not aligned: {} bytes after header",
            address_data.len()
        ));
    }

    let addresses: Vec<Pubkey> = address_data
        .chunks_exact(32)
        .map(|chunk| Pubkey::new_from_array(chunk.try_into().unwrap()))
        .collect();

    Ok(addresses)
}

fn main() -> Result<()> {
    dotenv::dotenv().ok();

    let rpc_url = std::env::var("RPC_URL")
        .map_err(|_| anyhow!("RPC_URL environment variable is required"))?;
    let keypair = load_keypair()?;
    let client = reqwest::blocking::Client::new();

    let desired = alt_addresses();
    eprintln!("ALT will contain {} addresses", desired.len());

    if let Ok(alt_addr_str) = std::env::var("ALT_ADDRESS") {
        // Extend existing ALT
        let alt_pubkey = Pubkey::from_str(&alt_addr_str)
            .map_err(|e| anyhow!("Invalid ALT_ADDRESS '{}': {}", alt_addr_str, e))?;
        eprintln!("Checking existing ALT: {}", alt_pubkey);

        let existing = fetch_alt_addresses(&client, &rpc_url, &alt_pubkey)?;
        eprintln!("ALT currently has {} addresses", existing.len());

        let missing: Vec<Pubkey> = desired
            .iter()
            .filter(|addr| !existing.contains(addr))
            .cloned()
            .collect();

        if missing.is_empty() {
            eprintln!("All {} addresses already present. Nothing to do.", desired.len());
        } else {
            eprintln!("Extending ALT with {} missing addresses", missing.len());

            let blockhash = get_latest_blockhash(&client, &rpc_url)?;
            let extend_ix = alt_ix::extend_lookup_table(
                alt_pubkey,
                keypair.pubkey(),
                Some(keypair.pubkey()),
                missing,
            );
            let extend_tx = Transaction::new_signed_with_payer(
                &[extend_ix],
                Some(&keypair.pubkey()),
                &[&keypair],
                blockhash,
            );
            let sig = send_transaction(&client, &rpc_url, &extend_tx)?;
            eprintln!("Extend TX sent: {}", sig);
            eprintln!("Waiting 2 seconds for confirmation...");
            std::thread::sleep(std::time::Duration::from_secs(2));
        }

        println!("ALT_ADDRESS={}", alt_pubkey);
    } else {
        // Create new ALT
        eprintln!("No ALT_ADDRESS set. Creating new ALT...");

        let recent_slot = get_slot(&client, &rpc_url)?;
        eprintln!("Recent slot: {}", recent_slot);

        let (create_ix, alt_pubkey) = alt_ix::create_lookup_table(
            keypair.pubkey(),
            keypair.pubkey(),
            recent_slot,
        );
        eprintln!("ALT address will be: {}", alt_pubkey);

        // Send create TX
        let blockhash = get_latest_blockhash(&client, &rpc_url)?;
        let create_tx = Transaction::new_signed_with_payer(
            &[create_ix],
            Some(&keypair.pubkey()),
            &[&keypair],
            blockhash,
        );
        let sig = send_transaction(&client, &rpc_url, &create_tx)?;
        eprintln!("Create TX sent: {}", sig);
        eprintln!("Waiting 2 seconds for ALT creation...");
        std::thread::sleep(std::time::Duration::from_secs(2));

        // Send extend TX with all 17 addresses
        let blockhash = get_latest_blockhash(&client, &rpc_url)?;
        let extend_ix = alt_ix::extend_lookup_table(
            alt_pubkey,
            keypair.pubkey(),
            Some(keypair.pubkey()),
            desired,
        );
        let extend_tx = Transaction::new_signed_with_payer(
            &[extend_ix],
            Some(&keypair.pubkey()),
            &[&keypair],
            blockhash,
        );
        let sig = send_transaction(&client, &rpc_url, &extend_tx)?;
        eprintln!("Extend TX sent: {}", sig);
        eprintln!("Waiting 2 seconds for activation...");
        std::thread::sleep(std::time::Duration::from_secs(2));

        println!("ALT_ADDRESS={}", alt_pubkey);
    }

    eprintln!("Done. Add the ALT_ADDRESS to your .env file.");
    Ok(())
}
