use dashmap::DashMap;
use solana_sdk::pubkey::Pubkey;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::router::pool::{DexType, PoolState};

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

/// Thread-safe pool state cache using DashMap for lock-free reads.
///
/// Every pool we've seen gets cached here. The TTL determines how long
/// we trust a cached state before requiring a refresh. For backrun arb,
/// stale state = wrong profit calculation = missed opportunity (or worse,
/// a bundle that reverts). Keep TTL tight — 1 slot (~400ms) is ideal.
#[derive(Clone)]
pub struct StateCache {
    pools: Arc<DashMap<PoolKey, CacheEntry>>,
    /// Index: token_mint -> list of pools containing that token
    token_to_pools: Arc<DashMap<Pubkey, Vec<Pubkey>>>,
    /// Index: vault_address -> (pool_address, is_token_a_vault)
    /// Populated during pool bootstrapping. Geyser gives us vault updates —
    /// this index tells us which pool was affected and which side changed.
    vault_to_pool: Arc<DashMap<Pubkey, (Pubkey, bool)>>,
    ttl: Duration,
}

impl StateCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            pools: Arc::new(DashMap::with_capacity(10_000)),
            token_to_pools: Arc::new(DashMap::with_capacity(5_000)),
            vault_to_pool: Arc::new(DashMap::with_capacity(20_000)),
            ttl,
        }
    }

    /// Insert or update a pool's state, refreshing the TTL.
    pub fn upsert(&self, pool_address: Pubkey, state: PoolState) {
        // Update token index
        for mint in &[state.token_a_mint, state.token_b_mint] {
            self.token_to_pools
                .entry(*mint)
                .and_modify(|pools| {
                    if !pools.contains(&pool_address) {
                        pools.push(pool_address);
                    }
                })
                .or_insert_with(|| vec![pool_address]);
        }

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
            .map(|v| v.value().clone())
            .unwrap_or_default()
    }

    /// Find pools that share a trading pair (both mints present).
    pub fn pools_for_pair(&self, mint_a: &Pubkey, mint_b: &Pubkey) -> Vec<Pubkey> {
        let pools_a = self.pools_for_token(mint_a);
        let pools_b: std::collections::HashSet<Pubkey> =
            self.pools_for_token(mint_b).into_iter().collect();

        pools_a
            .into_iter()
            .filter(|p| pools_b.contains(p))
            .collect()
    }

    /// Total number of cached pools.
    pub fn len(&self) -> usize {
        self.pools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pools.is_empty()
    }

    /// Evict pools that haven't been updated in 10 minutes.
    /// This is intentionally lenient — bootstrapped pools start with stale timestamps
    /// but remain valid for route discovery and vault indexing until Geyser updates arrive.
    /// The strict TTL (400ms) is enforced in `get()` for the simulator's fresh-state check.
    pub fn evict_stale(&self) {
        const EVICTION_AGE: Duration = Duration::from_secs(600);
        self.pools.retain(|_, entry| entry.last_updated.elapsed() < EVICTION_AGE);
    }

    /// Register a vault address → pool mapping.
    /// Called during pool bootstrapping so Geyser vault updates can be routed
    /// to the correct pool.
    ///
    /// `vault_address` - the SPL Token account address of the pool's token vault
    /// `pool_address` - the pool this vault belongs to
    /// `is_token_a` - true if this is the token_a vault, false for token_b
    pub fn register_vault(&self, vault_address: Pubkey, pool_address: Pubkey, is_token_a: bool) {
        self.vault_to_pool.insert(vault_address, (pool_address, is_token_a));
    }

    /// Look up which pool a vault belongs to.
    /// Returns (pool_address, is_token_a_vault) or None if unknown.
    pub fn pool_for_vault(&self, vault_address: &Pubkey) -> Option<(Pubkey, bool)> {
        self.vault_to_pool.get(vault_address).map(|v| *v.value())
    }

    /// Update a pool's reserve from a Geyser vault balance change.
    /// Returns the pool address if the update was applied (for downstream routing).
    pub fn update_vault_balance(
        &self,
        vault_address: &Pubkey,
        new_balance: u64,
        slot: u64,
    ) -> Option<Pubkey> {
        let (pool_address, is_token_a) = self.pool_for_vault(vault_address)?;

        let key = PoolKey { address: pool_address };
        let mut entry = self.pools.get_mut(&key)?;
        let cache_entry = entry.value_mut();

        // Only apply if this update is from a newer or equal slot
        if slot < cache_entry.state.last_slot {
            return None;
        }

        if is_token_a {
            cache_entry.state.token_a_reserve = new_balance;
        } else {
            cache_entry.state.token_b_reserve = new_balance;
        }
        cache_entry.state.last_slot = slot;
        cache_entry.last_updated = Instant::now();

        Some(pool_address)
    }

    /// Get all pool addresses for a given DEX type.
    pub fn pools_by_dex(&self, dex_type: DexType) -> Vec<Pubkey> {
        self.pools
            .iter()
            .filter(|entry| entry.value().state.dex_type == dex_type)
            .map(|entry| entry.key().address)
            .collect()
    }
}
