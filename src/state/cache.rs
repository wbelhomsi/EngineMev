use dashmap::DashMap;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::router::pool::PoolState;

/// Cache key combining pool address for O(1) lookup
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PoolKey {
    pub address: Pubkey,
}

/// Cached entry with TTL tracking
#[derive(Debug, Clone)]
struct CacheEntry {
    state: PoolState,
    last_updated: Instant,
}

/// Normalize a token pair key so (A,B) and (B,A) map to the same entry.
#[inline]
fn normalize_pair(a: &Pubkey, b: &Pubkey) -> (Pubkey, Pubkey) {
    if a <= b {
        (*a, *b)
    } else {
        (*b, *a)
    }
}

/// Thread-safe pool state cache using DashMap for lock-free reads.
///
/// Every pool we've seen gets cached here. The TTL determines how long
/// we trust a cached state before requiring a refresh. For backrun arb,
/// stale state = wrong profit calculation = missed opportunity (or worse,
/// a bundle that reverts). Keep TTL tight — 1 slot (~400ms) is ideal.
#[derive(Clone)]
pub struct StateCache {
    pools: Arc<DashMap<PoolKey, CacheEntry>>,
    /// Index: token_mint -> set of pools containing that token.
    /// HashSet gives O(1) dedup on upsert instead of O(n) Vec::contains.
    token_to_pools: Arc<DashMap<Pubkey, HashSet<Pubkey>>>,
    /// Index: normalized (min_mint, max_mint) -> list of pools trading that pair.
    /// Direct O(1) lookup replaces O(n) set intersection in pools_for_pair().
    pair_to_pools: Arc<DashMap<(Pubkey, Pubkey), Vec<Pubkey>>>,
    /// Mint address → token program (SPL Token or Token-2022).
    /// Populated by async getAccountInfo lookups, read by bundle builder.
    mint_programs: Arc<DashMap<Pubkey, Pubkey>>,
    /// Sanctum LstStateList: mint → index in the on-chain list.
    /// Populated at startup by fetching the LstStateList account.
    lst_indices: Arc<DashMap<Pubkey, u32>>,
    ttl: Duration,
}

impl StateCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            pools: Arc::new(DashMap::with_capacity(10_000)),
            token_to_pools: Arc::new(DashMap::with_capacity(5_000)),
            pair_to_pools: Arc::new(DashMap::with_capacity(20_000)),
            mint_programs: Arc::new(DashMap::with_capacity(1_000)),
            lst_indices: Arc::new(DashMap::with_capacity(200)),
            ttl,
        }
    }

    /// Insert or update a pool's state, refreshing the TTL.
    pub fn upsert(&self, pool_address: Pubkey, state: PoolState) {
        // Update token index (HashSet gives O(1) dedup)
        for mint in &[state.token_a_mint, state.token_b_mint] {
            self.token_to_pools
                .entry(*mint)
                .and_modify(|pools| {
                    pools.insert(pool_address);
                })
                .or_insert_with(|| {
                    let mut s = HashSet::with_capacity(4);
                    s.insert(pool_address);
                    s
                });
        }

        // Update pair index with normalized key (min, max) to avoid (A,B)/(B,A) dupes
        let pair_key = normalize_pair(&state.token_a_mint, &state.token_b_mint);
        self.pair_to_pools
            .entry(pair_key)
            .and_modify(|pools| {
                if !pools.contains(&pool_address) {
                    pools.push(pool_address);
                }
            })
            .or_insert_with(|| vec![pool_address]);

        let key = PoolKey {
            address: pool_address,
        };
        self.pools.insert(
            key,
            CacheEntry {
                state,
                last_updated: Instant::now(),
            },
        );
    }

    /// Get a pool state if it exists and is within TTL.
    pub fn get(&self, pool_address: &Pubkey) -> Option<PoolState> {
        let key = PoolKey {
            address: *pool_address,
        };
        self.pools.get(&key).and_then(|entry| {
            if entry.last_updated.elapsed() < self.ttl {
                Some(entry.state.clone())
            } else {
                None
            }
        })
    }

    /// Get a pool state even if stale — useful for route discovery
    /// where approximate state is acceptable.
    pub fn get_any(&self, pool_address: &Pubkey) -> Option<PoolState> {
        let key = PoolKey {
            address: *pool_address,
        };
        self.pools.get(&key).map(|entry| entry.state.clone())
    }

    /// Find all pools that trade a given token.
    pub fn pools_for_token(&self, mint: &Pubkey) -> Vec<Pubkey> {
        self.token_to_pools
            .get(mint)
            .map(|v| v.value().iter().copied().collect())
            .unwrap_or_default()
    }

    /// Find pools that share a trading pair (both mints present).
    /// O(1) lookup via pre-computed pair index instead of O(n) set intersection.
    pub fn pools_for_pair(&self, mint_a: &Pubkey, mint_b: &Pubkey) -> Vec<Pubkey> {
        let pair_key = normalize_pair(mint_a, mint_b);
        self.pair_to_pools
            .get(&pair_key)
            .map(|v| v.value().clone())
            .unwrap_or_default()
    }

    /// Total number of cached pools.
    pub fn len(&self) -> usize {
        self.pools.len()
    }

    /// Returns true if the cache contains no pools.
    pub fn is_empty(&self) -> bool {
        self.pools.is_empty()
    }

    /// Get the token program for a mint (SPL Token or Token-2022).
    pub fn get_mint_program(&self, mint: &Pubkey) -> Option<Pubkey> {
        self.mint_programs.get(mint).map(|v| *v.value())
    }

    /// Set the token program for a mint.
    pub fn set_mint_program(&self, mint: Pubkey, program: Pubkey) {
        self.mint_programs.insert(mint, program);
    }

    /// Get the Sanctum LstStateList index for a given mint.
    pub fn get_lst_index(&self, mint: &Pubkey) -> Option<u32> {
        self.lst_indices.get(mint).map(|v| *v)
    }

    /// Set the Sanctum LstStateList index for a mint.
    pub fn set_lst_index(&self, mint: Pubkey, index: u32) {
        self.lst_indices.insert(mint, index);
    }

    /// Evict pools that haven't been updated in 10 minutes.
    /// This is intentionally lenient — bootstrapped pools start with stale timestamps
    /// but remain valid for route discovery and vault indexing until Geyser updates arrive.
    /// The strict TTL (400ms) is enforced in `get()` for the simulator's fresh-state check.
    pub fn evict_stale(&self) {
        const EVICTION_AGE: Duration = Duration::from_secs(600);

        // Collect stale pools and their mints before removing
        let mut stale: Vec<(Pubkey, Pubkey, Pubkey)> = Vec::new();
        self.pools.retain(|key, entry| {
            let keep = entry.last_updated.elapsed() < EVICTION_AGE;
            if !keep {
                stale.push((
                    key.address,
                    entry.state.token_a_mint,
                    entry.state.token_b_mint,
                ));
            }
            keep
        });

        // Clean token_to_pools and pair_to_pools indices for evicted pools
        for (pool_addr, mint_a, mint_b) in &stale {
            // Remove from token index
            for mint in &[mint_a, mint_b] {
                if let Some(mut entry) = self.token_to_pools.get_mut(mint) {
                    entry.value_mut().remove(pool_addr);
                    if entry.value().is_empty() {
                        drop(entry);
                        self.token_to_pools.remove(mint);
                    }
                }
            }

            // Remove from pair index
            let pair_key = normalize_pair(mint_a, mint_b);
            if let Some(mut entry) = self.pair_to_pools.get_mut(&pair_key) {
                entry.value_mut().retain(|p| p != pool_addr);
                if entry.value().is_empty() {
                    drop(entry);
                    self.pair_to_pools.remove(&pair_key);
                }
            }
        }
    }

}
