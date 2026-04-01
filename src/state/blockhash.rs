use solana_sdk::hash::Hash;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::{info, warn, error};

const STALE_THRESHOLD: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct BlockhashInfo {
    pub blockhash: Hash,
    pub last_valid_block_height: u64,
    pub fetched_at: Instant,
}

#[derive(Clone)]
pub struct BlockhashCache {
    inner: Arc<RwLock<Option<BlockhashInfo>>>,
}

impl BlockhashCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
        }
    }

    pub fn update(&self, info: BlockhashInfo) {
        match self.inner.write() {
            Ok(mut guard) => *guard = Some(info),
            Err(poisoned) => *poisoned.into_inner() = Some(info),
        }
    }

    pub fn get(&self) -> Option<Hash> {
        let guard = match self.inner.read() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.as_ref().and_then(|info| {
            if info.fetched_at.elapsed() < STALE_THRESHOLD {
                Some(info.blockhash)
            } else {
                None
            }
        })
    }
}

/// Fetch the latest blockhash from RPC and update the cache.
pub async fn fetch_and_update(
    client: &reqwest::Client,
    rpc_url: &str,
    cache: &BlockhashCache,
) -> anyhow::Result<()> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getLatestBlockhash",
        "params": [{ "commitment": "confirmed" }]
    });

    let resp = client
        .post(rpc_url)
        .json(&payload)
        .timeout(Duration::from_secs(5))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let value = resp
        .get("result")
        .and_then(|r| r.get("value"))
        .ok_or_else(|| anyhow::anyhow!("Missing result.value in getLatestBlockhash response"))?;

    let blockhash_str = value
        .get("blockhash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing blockhash field"))?;

    let last_valid_block_height = value
        .get("lastValidBlockHeight")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow::anyhow!("Missing lastValidBlockHeight field"))?;

    let blockhash: Hash = blockhash_str.parse()
        .map_err(|_| anyhow::anyhow!("Invalid blockhash: {}", blockhash_str))?;

    cache.update(BlockhashInfo {
        blockhash,
        last_valid_block_height,
        fetched_at: Instant::now(),
    });

    Ok(())
}

/// Spawn the background blockhash refresh loop.
pub async fn run_blockhash_loop(
    client: reqwest::Client,
    rpc_url: String,
    cache: BlockhashCache,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut consecutive_failures: u32 = 0;

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { break; }
            }
            _ = tokio::time::sleep(Duration::from_secs(2)) => {
                match fetch_and_update(&client, &rpc_url, &cache).await {
                    Ok(()) => {
                        if consecutive_failures > 0 {
                            info!("Blockhash fetch recovered after {} failures", consecutive_failures);
                        }
                        consecutive_failures = 0;
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        if consecutive_failures >= 3 {
                            error!("Blockhash fetch failed {} times: {}", consecutive_failures, e);
                        } else {
                            warn!("Blockhash fetch failed ({}x): {}", consecutive_failures, e);
                        }
                    }
                }
            }
        }
    }

    info!("Blockhash refresh loop exited");
}
