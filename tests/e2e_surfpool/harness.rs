use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use serde_json::{json, Value};
use solana_sdk::{
    hash::Hash,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::{Keypair, SeedDerivable},
    signer::Signer,
    transaction::Transaction,
};

/// Result of sending a transaction to Surfpool.
pub struct TxResult {
    pub signature: String,
    pub success: bool,
    pub logs: Vec<String>,
    pub error: Option<String>,
}

/// Manages a Surfpool subprocess and provides RPC helpers for tests.
pub struct SurfpoolHarness {
    process: Option<Child>,
    rpc_url: String,
    client: Client,
}

impl SurfpoolHarness {
    /// Deterministic test keypair (64 bytes: 32-byte secret + 32-byte pubkey derived from it).
    /// Using a fixed seed so the airdrop address is predictable.
    pub fn test_keypair() -> Keypair {
        let seed: [u8; 32] = [
            42, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
            23, 24, 25, 26, 27, 28, 29, 30, 31,
        ];
        Keypair::from_seed(&seed).expect("Failed to create keypair from seed")
    }

    /// Start Surfpool subprocess, wait until healthy, return harness.
    ///
    /// Requires `RPC_URL` env var (mainnet RPC for forking).
    /// Requires `surfpool` binary on PATH.
    pub fn start() -> Self {
        let upstream_rpc =
            std::env::var("RPC_URL").expect("RPC_URL env var required for Surfpool tests");

        let signer = Self::test_keypair();
        let signer_pubkey = signer.pubkey().to_string();

        let port = 18900u16;
        let ws_port = 18901u16;
        let rpc_url = format!("http://127.0.0.1:{}", port);

        println!("[surfpool-harness] Starting surfpool on port {}...", port);
        println!("[surfpool-harness] Signer pubkey: {}", signer_pubkey);
        println!(
            "[surfpool-harness] Upstream RPC: {}...{}",
            &upstream_rpc[..upstream_rpc.len().min(30)],
            if upstream_rpc.len() > 30 {
                "(redacted)"
            } else {
                ""
            }
        );

        // Spawn surfpool subprocess
        let child = Command::new("surfpool")
            .arg("start")
            .arg("--rpc-url")
            .arg(&upstream_rpc)
            .arg("--ci")
            .arg("--port")
            .arg(port.to_string())
            .arg("--ws-port")
            .arg(ws_port.to_string())
            .arg("--airdrop")
            .arg(&signer_pubkey)
            .arg("--airdrop-amount")
            .arg("100000000000") // 100 SOL in lamports
            .arg("--no-deploy")
            .env("NO_DNA", "1")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Failed to spawn surfpool. Is it installed and on PATH?");

        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to build reqwest client");

        let harness = Self {
            process: Some(child),
            rpc_url,
            client,
        };

        // Wait for surfpool to become healthy
        harness.wait_for_ready();

        // Give surfpool a few extra seconds after health check to finish account cloning
        println!("[surfpool-harness] Health check passed, waiting 3s for account cloning...");
        std::thread::sleep(Duration::from_secs(3));

        println!("[surfpool-harness] Ready.");
        harness
    }

    /// Poll `getHealth` every 500ms until it returns "ok" or timeout (30s).
    fn wait_for_ready(&self) {
        let start = Instant::now();
        let timeout = Duration::from_secs(30);
        let poll_interval = Duration::from_millis(500);

        loop {
            if start.elapsed() > timeout {
                panic!(
                    "[surfpool-harness] Timed out after {:?} waiting for surfpool health check at {}",
                    timeout, self.rpc_url
                );
            }

            if self.check_health() {
                return;
            }

            std::thread::sleep(poll_interval);
        }
    }

    /// Returns true if `getHealth` returns successfully.
    fn check_health(&self) -> bool {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getHealth",
        });

        match self.client.post(&self.rpc_url).json(&body).send() {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    /// Check if surfpool is currently healthy.
    pub fn is_ready(&self) -> bool {
        self.check_health()
    }

    /// The local RPC URL for this Surfpool instance.
    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    pub fn client(&self) -> &reqwest::blocking::Client {
        &self.client
    }

    /// Get the latest blockhash from Surfpool.
    pub fn get_latest_blockhash(&self) -> Hash {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getLatestBlockhash",
            "params": [{"commitment": "finalized"}],
        });

        let resp: Value = self
            .client
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .expect("getLatestBlockhash request failed")
            .json()
            .expect("getLatestBlockhash response not JSON");

        let hash_str = resp["result"]["value"]["blockhash"]
            .as_str()
            .expect("No blockhash in response");

        hash_str
            .parse::<Hash>()
            .expect("Failed to parse blockhash")
    }

    /// Send a transaction with the given instructions, signed by the given keypair.
    /// Returns a TxResult with signature, success status, logs, and any error.
    pub fn send_tx(&self, instructions: &[Instruction], signer: &Keypair) -> TxResult {
        let blockhash = self.get_latest_blockhash();

        let tx = Transaction::new_signed_with_payer(
            instructions,
            Some(&signer.pubkey()),
            &[signer],
            blockhash,
        );

        let serialized =
            bincode::serialize(&tx).expect("Failed to serialize transaction");

        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&serialized);

        // Send with skipPreflight and base64 encoding
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendTransaction",
            "params": [
                encoded,
                {
                    "skipPreflight": true,
                    "encoding": "base64",
                }
            ],
        });

        let resp: Value = self
            .client
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .expect("sendTransaction request failed")
            .json()
            .expect("sendTransaction response not JSON");

        // Check for RPC-level error
        if let Some(err) = resp.get("error") {
            return TxResult {
                signature: String::new(),
                success: false,
                logs: Vec::new(),
                error: Some(format!("{}", err)),
            };
        }

        let signature = resp["result"]
            .as_str()
            .unwrap_or("")
            .to_string();

        if signature.is_empty() {
            return TxResult {
                signature,
                success: false,
                logs: Vec::new(),
                error: Some(format!("No signature in response: {}", resp)),
            };
        }

        // Wait a moment for the tx to be processed, then fetch it
        std::thread::sleep(Duration::from_millis(2000));

        // Fetch the transaction to get logs
        let (success, logs, error) = self.get_tx_status(&signature);

        TxResult {
            signature,
            success,
            logs,
            error,
        }
    }

    /// Fetch a confirmed transaction and extract logs + status.
    fn get_tx_status(&self, signature: &str) -> (bool, Vec<String>, Option<String>) {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getTransaction",
            "params": [
                signature,
                {
                    "encoding": "json",
                    "maxSupportedTransactionVersion": 0,
                    "commitment": "confirmed",
                }
            ],
        });

        let resp: Value = match self.client.post(&self.rpc_url).json(&body).send() {
            Ok(r) => match r.json() {
                Ok(v) => v,
                Err(e) => {
                    return (
                        false,
                        Vec::new(),
                        Some(format!("getTransaction parse error: {}", e)),
                    )
                }
            },
            Err(e) => {
                return (
                    false,
                    Vec::new(),
                    Some(format!("getTransaction request error: {}", e)),
                )
            }
        };

        if resp["result"].is_null() {
            return (
                false,
                Vec::new(),
                Some("Transaction not found (null result)".to_string()),
            );
        }

        let logs: Vec<String> = resp["result"]["meta"]["logMessages"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let err = &resp["result"]["meta"]["err"];
        if err.is_null() {
            (true, logs, None)
        } else {
            (false, logs, Some(format!("{}", err)))
        }
    }

    /// Get SOL balance for a pubkey (in lamports).
    pub fn get_sol_balance(&self, pubkey: &Pubkey) -> u64 {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getBalance",
            "params": [pubkey.to_string()],
        });

        let resp: Value = self
            .client
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .expect("getBalance request failed")
            .json()
            .expect("getBalance response not JSON");

        resp["result"]["value"]
            .as_u64()
            .unwrap_or(0)
    }

    /// Get SPL token balance for an owner + mint pair.
    /// Returns 0 if no token account exists.
    pub fn get_token_balance(&self, owner: &Pubkey, mint: &Pubkey) -> u64 {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getTokenAccountsByOwner",
            "params": [
                owner.to_string(),
                {"mint": mint.to_string()},
                {"encoding": "jsonParsed"},
            ],
        });

        let resp: Value = self
            .client
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .expect("getTokenAccountsByOwner request failed")
            .json()
            .expect("getTokenAccountsByOwner response not JSON");

        let accounts = match resp["result"]["value"].as_array() {
            Some(arr) => arr,
            None => return 0,
        };

        if accounts.is_empty() {
            return 0;
        }

        // Parse the first account's tokenAmount
        let amount_str = accounts[0]["account"]["data"]["parsed"]["info"]["tokenAmount"]["amount"]
            .as_str()
            .unwrap_or("0");

        amount_str.parse::<u64>().unwrap_or(0)
    }

    /// Get raw account data for a pubkey. Returns None if account doesn't exist.
    pub fn get_account_data(&self, pubkey: &Pubkey) -> Option<Vec<u8>> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getAccountInfo",
            "params": [
                pubkey.to_string(),
                {"encoding": "base64"},
            ],
        });

        let resp: Value = self
            .client
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .ok()?
            .json()
            .ok()?;

        let data_arr = resp["result"]["value"]["data"].as_array()?;
        let b64_str = data_arr.first()?.as_str()?;

        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(b64_str)
            .ok()
    }
}

impl Drop for SurfpoolHarness {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.process {
            println!("[surfpool-harness] Killing surfpool subprocess (pid={})...", child.id());
            let _ = child.kill();
            let _ = child.wait();
            println!("[surfpool-harness] Surfpool subprocess terminated.");
        }
    }
}
