use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use std::io::Write;
use std::sync::Mutex;

/// Global mutex to serialize tests that touch env vars (env vars are process-global).
static ENV_MUTEX: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// load_keypair — JSON file path
// ---------------------------------------------------------------------------

#[test]
fn test_load_keypair_from_json_file() {
    let _lock = ENV_MUTEX.lock().unwrap();
    // Ensure env var is NOT set so the file path is used.
    std::env::remove_var("SEARCHER_PRIVATE_KEY");

    let kp = Keypair::new();
    let bytes_vec: Vec<u8> = kp.to_bytes().to_vec();
    let json = serde_json::to_string(&bytes_vec).unwrap();

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(json.as_bytes()).unwrap();
    tmp.flush().unwrap();

    let loaded = solana_mev_bot::rpc_helpers::load_keypair(tmp.path().to_str().unwrap())
        .expect("should load keypair from valid JSON file");

    assert_eq!(loaded.pubkey(), kp.pubkey());
}

#[test]
fn test_load_keypair_from_json_file_preserves_secret() {
    let _lock = ENV_MUTEX.lock().unwrap();
    std::env::remove_var("SEARCHER_PRIVATE_KEY");

    let kp = Keypair::new();
    let bytes_vec: Vec<u8> = kp.to_bytes().to_vec();
    let json = serde_json::to_string(&bytes_vec).unwrap();

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(json.as_bytes()).unwrap();
    tmp.flush().unwrap();

    let loaded = solana_mev_bot::rpc_helpers::load_keypair(tmp.path().to_str().unwrap()).unwrap();

    // Full 64-byte secret key must round-trip.
    assert_eq!(loaded.to_bytes(), kp.to_bytes());
}

#[test]
fn test_load_keypair_invalid_json() {
    let _lock = ENV_MUTEX.lock().unwrap();
    std::env::remove_var("SEARCHER_PRIVATE_KEY");

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(b"not valid json at all").unwrap();
    tmp.flush().unwrap();

    let result = solana_mev_bot::rpc_helpers::load_keypair(tmp.path().to_str().unwrap());
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Invalid keypair JSON"),
        "unexpected error message: {msg}"
    );
}

#[test]
fn test_load_keypair_wrong_byte_length() {
    let _lock = ENV_MUTEX.lock().unwrap();
    std::env::remove_var("SEARCHER_PRIVATE_KEY");

    // Valid JSON array but only 32 bytes — Keypair needs 64.
    let short: Vec<u8> = vec![1u8; 32];
    let json = serde_json::to_string(&short).unwrap();

    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(json.as_bytes()).unwrap();
    tmp.flush().unwrap();

    let result = solana_mev_bot::rpc_helpers::load_keypair(tmp.path().to_str().unwrap());
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Invalid keypair bytes"),
        "unexpected error message: {msg}"
    );
}

#[test]
fn test_load_keypair_file_not_found() {
    let _lock = ENV_MUTEX.lock().unwrap();
    std::env::remove_var("SEARCHER_PRIVATE_KEY");

    let result =
        solana_mev_bot::rpc_helpers::load_keypair("/tmp/does_not_exist_9f8a7b6c.json");
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Failed to read keypair file"),
        "unexpected error message: {msg}"
    );
}

// ---------------------------------------------------------------------------
// load_keypair — SEARCHER_PRIVATE_KEY env var (base58)
// ---------------------------------------------------------------------------

#[test]
fn test_load_keypair_from_env_base58() {
    let _lock = ENV_MUTEX.lock().unwrap();

    let kp = Keypair::new();
    let b58 = bs58::encode(kp.to_bytes()).into_string();
    std::env::set_var("SEARCHER_PRIVATE_KEY", &b58);

    // Pass a dummy file path — env var should take precedence.
    let loaded = solana_mev_bot::rpc_helpers::load_keypair("/tmp/nonexistent_placeholder.json")
        .expect("should load keypair from SEARCHER_PRIVATE_KEY env var");

    assert_eq!(loaded.pubkey(), kp.pubkey());
    assert_eq!(loaded.to_bytes(), kp.to_bytes());

    std::env::remove_var("SEARCHER_PRIVATE_KEY");
}

#[test]
fn test_load_keypair_env_takes_precedence_over_file() {
    let _lock = ENV_MUTEX.lock().unwrap();

    // Create a valid file keypair.
    let file_kp = Keypair::new();
    let file_json = serde_json::to_string(&file_kp.to_bytes().to_vec()).unwrap();
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(file_json.as_bytes()).unwrap();
    tmp.flush().unwrap();

    // Set env var to a DIFFERENT keypair.
    let env_kp = Keypair::new();
    let b58 = bs58::encode(env_kp.to_bytes()).into_string();
    std::env::set_var("SEARCHER_PRIVATE_KEY", &b58);

    let loaded =
        solana_mev_bot::rpc_helpers::load_keypair(tmp.path().to_str().unwrap()).unwrap();

    // Must match the env keypair, NOT the file keypair.
    assert_eq!(loaded.pubkey(), env_kp.pubkey());
    assert_ne!(loaded.pubkey(), file_kp.pubkey());

    std::env::remove_var("SEARCHER_PRIVATE_KEY");
}

#[test]
fn test_load_keypair_env_invalid_base58() {
    let _lock = ENV_MUTEX.lock().unwrap();

    std::env::set_var("SEARCHER_PRIVATE_KEY", "not!valid!base58!!!");

    let result =
        solana_mev_bot::rpc_helpers::load_keypair("/tmp/nonexistent_placeholder.json");
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Invalid base58 SEARCHER_PRIVATE_KEY"),
        "unexpected error message: {msg}"
    );

    std::env::remove_var("SEARCHER_PRIVATE_KEY");
}

#[test]
fn test_load_keypair_env_valid_base58_wrong_length() {
    let _lock = ENV_MUTEX.lock().unwrap();

    // Valid base58 but only 16 bytes — far too short for a keypair.
    let short_b58 = bs58::encode(&[0xABu8; 16]).into_string();
    std::env::set_var("SEARCHER_PRIVATE_KEY", &short_b58);

    let result =
        solana_mev_bot::rpc_helpers::load_keypair("/tmp/nonexistent_placeholder.json");
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Invalid keypair bytes"),
        "unexpected error message: {msg}"
    );

    std::env::remove_var("SEARCHER_PRIVATE_KEY");
}

#[test]
fn test_load_keypair_env_with_whitespace_trimmed() {
    let _lock = ENV_MUTEX.lock().unwrap();

    let kp = Keypair::new();
    let b58 = bs58::encode(kp.to_bytes()).into_string();
    // Surround with whitespace — load_keypair trims.
    std::env::set_var("SEARCHER_PRIVATE_KEY", format!("  {b58}  \n"));

    let loaded = solana_mev_bot::rpc_helpers::load_keypair("/tmp/nonexistent.json")
        .expect("should handle whitespace around base58 key");
    assert_eq!(loaded.pubkey(), kp.pubkey());

    std::env::remove_var("SEARCHER_PRIVATE_KEY");
}
