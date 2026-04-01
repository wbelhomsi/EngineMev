use std::time::{Duration, Instant};
use solana_sdk::hash::Hash;

use solana_mev_bot::state::blockhash::{BlockhashCache, BlockhashInfo};

#[test]
fn test_blockhash_cache_returns_fresh() {
    let cache = BlockhashCache::new();
    let hash = Hash::new_unique();
    cache.update(BlockhashInfo {
        blockhash: hash,
        last_valid_block_height: 1000,
        fetched_at: Instant::now(),
    });
    assert_eq!(cache.get(), Some(hash));
}

#[test]
fn test_blockhash_cache_returns_none_when_empty() {
    let cache = BlockhashCache::new();
    assert_eq!(cache.get(), None);
}

#[test]
fn test_blockhash_cache_returns_none_when_stale() {
    let cache = BlockhashCache::new();
    let hash = Hash::new_unique();
    cache.update(BlockhashInfo {
        blockhash: hash,
        last_valid_block_height: 1000,
        fetched_at: Instant::now() - Duration::from_secs(10),
    });
    assert_eq!(cache.get(), None, "Stale blockhash should return None");
}

#[test]
fn test_blockhash_cache_clone_shares_state() {
    let cache1 = BlockhashCache::new();
    let cache2 = cache1.clone();
    let hash = Hash::new_unique();
    cache1.update(BlockhashInfo {
        blockhash: hash,
        last_valid_block_height: 1000,
        fetched_at: Instant::now(),
    });
    assert_eq!(cache2.get(), Some(hash), "Cloned cache should see update");
}
