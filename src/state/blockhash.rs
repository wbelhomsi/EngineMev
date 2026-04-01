use solana_sdk::hash::Hash;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

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
        let mut guard = self.inner.write().unwrap();
        *guard = Some(info);
    }

    pub fn get(&self) -> Option<Hash> {
        let guard = self.inner.read().unwrap();
        guard.as_ref().and_then(|info| {
            if info.fetched_at.elapsed() < STALE_THRESHOLD {
                Some(info.blockhash)
            } else {
                None
            }
        })
    }
}
