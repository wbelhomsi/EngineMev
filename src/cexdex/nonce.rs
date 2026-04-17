//! Durable-nonce pool for cexdex multi-relay fan-out.
//!
//! Wraps a fixed set of nonce accounts (from CEXDEX_SEARCHER_NONCE_ACCOUNTS).
//! Each bundle dispatch checks out a nonce and its current on-chain blockhash;
//! the confirmation tracker calls mark_settled on every terminal state.
//! Geyser's nonce parser calls update_cached_hash whenever the on-chain value
//! changes (landings, external advances, or bootstrap fetch).
//!
//! Selection policy: round-robin by last_used (oldest first); config order
//! breaks ties. On collision (checkout picks an in-flight nonce), we return
//! the pubkey anyway and increment cexdex_nonce_collision_total — the
//! caller's tx will fail safely at the on-chain nonce check.

use dashmap::DashMap;
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use std::sync::Arc;
use std::time::Instant;

/// Pass-through tuple used by the bundle builder to construct the
/// advance_nonce_account ix.
#[derive(Debug, Clone, Copy)]
pub struct NonceInfo {
    pub account: Pubkey,
    pub authority: Pubkey,
}

#[derive(Debug, Clone)]
struct NonceState {
    /// For round-robin selection. Updated on checkout.
    last_used: Instant,
    /// True from checkout until mark_settled. Used for collision counting.
    in_flight: bool,
    /// Last on-chain blockhash we've observed, written by update_cached_hash.
    /// `Some(hash)` once Geyser (or the startup RPC bootstrap) has populated it.
    cached_hash: Option<Hash>,
    /// Wall-clock time the hash was last updated; exposed via metrics only.
    hash_observed_at: Option<Instant>,
}

impl NonceState {
    fn new(epoch: Instant) -> Self {
        Self {
            last_used: epoch,
            in_flight: false,
            cached_hash: None,
            hash_observed_at: None,
        }
    }
}

/// Thread-safe, cloneable pool.
#[derive(Clone)]
pub struct NoncePool {
    nonces: Arc<Vec<Pubkey>>,
    state: Arc<DashMap<Pubkey, NonceState>>,
}

impl NoncePool {
    pub fn new(nonces: Vec<Pubkey>) -> Self {
        let state = DashMap::new();
        let epoch = Instant::now();
        for pk in &nonces {
            state.insert(*pk, NonceState::new(epoch));
        }
        Self {
            nonces: Arc::new(nonces),
            state: Arc::new(state),
        }
    }

    /// # Concurrency
    ///
    /// Must be called from a single task. The read (winner selection) and write
    /// (in_flight = true) phases are NOT atomic; concurrent callers could both
    /// win the same nonce without triggering the collision counter. The detector
    /// loop is single-threaded so contention is zero in practice — but if this
    /// invariant changes, wrap callers in a Mutex or rewrite the selection under
    /// a single DashMap entry transaction.
    ///
    /// Round-robin by last_used (oldest first); config order breaks ties.
    /// Returns None if no nonce has a cached_hash yet (pre-Geyser warmup —
    /// detector should skip). On collision (winner is in_flight), still returns
    /// the winner and increments cexdex_nonce_collision_total.
    pub fn checkout(&self) -> Option<(Pubkey, Hash)> {
        // Winner: prefer !in_flight over in_flight, then oldest last_used,
        // then config order. Skip any nonce without a cached hash.
        let mut best: Option<(usize, Pubkey, NonceState)> = None;
        for (idx, pk) in self.nonces.iter().enumerate() {
            let entry = self.state.get(pk).expect("pk must be in state");
            if entry.cached_hash.is_none() {
                continue;
            }
            let candidate = (idx, *pk, entry.value().clone());
            best = match best {
                None => Some(candidate),
                Some(ref cur) => {
                    match (cur.2.in_flight, candidate.2.in_flight) {
                        (true, false) => Some(candidate),
                        (false, true) => Some(cur.clone()),
                        _ => {
                            if candidate.2.last_used < cur.2.last_used
                                || (candidate.2.last_used == cur.2.last_used
                                    && candidate.0 < cur.0)
                            {
                                Some(candidate)
                            } else {
                                Some(cur.clone())
                            }
                        }
                    }
                }
            };
        }

        let (_, pk, prev_state) = best?;
        let hash = prev_state.cached_hash?;
        if prev_state.in_flight {
            crate::metrics::counters::inc_cexdex_nonce_collision_total();
        }

        let mut entry = self.state.get_mut(&pk).expect("pk must be in state");
        entry.last_used = Instant::now();
        entry.in_flight = true;
        drop(entry);
        let in_flight_count = self.state.iter().filter(|e| e.in_flight).count();
        crate::metrics::counters::set_cexdex_nonce_in_flight(in_flight_count);

        Some((pk, hash))
    }

    /// Clear in_flight — call from confirmation tracker (Landed / Failed /
    /// Timeout / RpcError exhaustion).
    pub fn mark_settled(&self, pubkey: Pubkey) {
        if let Some(mut entry) = self.state.get_mut(&pubkey) {
            entry.in_flight = false;
        }
        let in_flight_count = self.state.iter().filter(|e| e.in_flight).count();
        crate::metrics::counters::set_cexdex_nonce_in_flight(in_flight_count);
    }

    /// Geyser-driven update. Called whenever an on-chain nonce account
    /// changes (bundle landings, external advances, startup bootstrap).
    pub fn update_cached_hash(&self, pubkey: Pubkey, hash: Hash) {
        if let Some(mut entry) = self.state.get_mut(&pubkey) {
            entry.cached_hash = Some(hash);
            entry.hash_observed_at = Some(Instant::now());
            crate::metrics::counters::inc_cexdex_nonce_hash_refresh_total();
        }
    }

    /// True if `pubkey` is one of the managed nonce accounts.
    /// Used by the Geyser parser dispatch to short-circuit pool parsers.
    pub fn contains(&self, pubkey: &Pubkey) -> bool {
        self.state.contains_key(pubkey)
    }

    /// Total number of nonce accounts.
    pub fn len(&self) -> usize {
        self.nonces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nonces.is_empty()
    }

    /// Iterator over all managed pubkeys, used for startup validation.
    pub fn pubkeys(&self) -> impl Iterator<Item = Pubkey> + '_ {
        self.nonces.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::hash::Hash;
    use solana_sdk::pubkey::Pubkey;

    fn fake_hash(byte: u8) -> Hash {
        Hash::new_from_array([byte; 32])
    }

    #[test]
    fn returns_none_before_cache_populated() {
        let pool = NoncePool::new(vec![Pubkey::new_unique(), Pubkey::new_unique()]);
        assert!(pool.checkout().is_none());
    }

    #[test]
    fn checkout_returns_cached_hash() {
        let a = Pubkey::new_unique();
        let pool = NoncePool::new(vec![a]);
        pool.update_cached_hash(a, fake_hash(0x11));
        let (pk, h) = pool.checkout().expect("checkout should succeed");
        assert_eq!(pk, a);
        assert_eq!(h, fake_hash(0x11));
    }

    #[test]
    fn round_robin_picks_oldest() {
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();
        let c = Pubkey::new_unique();
        let pool = NoncePool::new(vec![a, b, c]);
        pool.update_cached_hash(a, fake_hash(0xAA));
        pool.update_cached_hash(b, fake_hash(0xBB));
        pool.update_cached_hash(c, fake_hash(0xCC));

        let (first, _) = pool.checkout().unwrap();
        pool.mark_settled(first);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let (second, _) = pool.checkout().unwrap();
        pool.mark_settled(second);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let (third, _) = pool.checkout().unwrap();

        assert_ne!(first, second);
        assert_ne!(second, third);
        assert_ne!(first, third);
    }

    #[test]
    fn collision_when_all_in_flight() {
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();
        let pool = NoncePool::new(vec![a, b]);
        pool.update_cached_hash(a, fake_hash(0x01));
        pool.update_cached_hash(b, fake_hash(0x02));

        let _ = pool.checkout().unwrap();
        let _ = pool.checkout().unwrap();
        // Third checkout: both in-flight; collision counter fires internally but
        // we still return a valid nonce.
        let third = pool.checkout();
        assert!(third.is_some(), "checkout should not return None when cache populated");
    }

    #[test]
    fn mark_settled_clears_in_flight() {
        let a = Pubkey::new_unique();
        let pool = NoncePool::new(vec![a]);
        pool.update_cached_hash(a, fake_hash(0xFF));
        let (pk, _) = pool.checkout().unwrap();
        pool.mark_settled(pk);
        let (pk2, _) = pool.checkout().unwrap();
        assert_eq!(pk, pk2);
    }

    #[test]
    fn contains_returns_true_for_managed_pubkeys() {
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();
        let pool = NoncePool::new(vec![a, b]);
        assert!(pool.contains(&a));
        assert!(pool.contains(&b));
        assert!(!pool.contains(&Pubkey::new_unique()));
    }

    #[test]
    fn update_cached_hash_overwrites() {
        let a = Pubkey::new_unique();
        let pool = NoncePool::new(vec![a]);
        pool.update_cached_hash(a, fake_hash(0x11));
        pool.update_cached_hash(a, fake_hash(0x22));
        let (_, h) = pool.checkout().unwrap();
        assert_eq!(h, fake_hash(0x22));
    }

    #[test]
    fn checkout_skips_uncached_nonces() {
        let a = Pubkey::new_unique();
        let b = Pubkey::new_unique();
        let pool = NoncePool::new(vec![a, b]);
        // Only the second has a cached hash
        pool.update_cached_hash(b, fake_hash(0xBB));
        let (pk, h) = pool.checkout().expect("should return the only cached nonce");
        assert_eq!(pk, b);
        assert_eq!(h, fake_hash(0xBB));
    }
}
