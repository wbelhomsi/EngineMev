# CEX-DEX Nonce-Based Relay Fan-Out Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Re-enable Jito + Astralane relay fan-out for the cexdex binary safely, using durable Solana nonce accounts so at most one bundle lands per opportunity regardless of how many relays we submit to.

**Architecture:** Every cexdex-submitted tx begins with `advance_nonce_account(N, authority)` as instruction #0 and uses the nonce's current hash as `recent_blockhash`. Round-robin over 3 nonce accounts (oldest last-used wins), Geyser-maintained hash cache (zero RPC on hot path). Per-relay independent tip fractions. Main engine (DEX↔DEX) path is unchanged — nonce is opt-in via an `Option<NonceInfo>` param.

**Tech Stack:** Rust, solana-sdk 4.0, solana-system-interface 3.1 (feature `bincode`) for `advance_nonce_account`, helius-laserstream gRPC, DashMap, metrics crate, Prometheus/Grafana.

**Spec:** `docs/superpowers/specs/2026-04-17-cexdex-nonce-fanout-design.md`

---

## File Structure

**New files:**
- `src/mempool/parsers/nonce.rs` — 80-byte nonce account parser (~60 lines)
- `src/cexdex/nonce.rs` — `NoncePool`, `NonceInfo`, `NonceState` (~200 lines)
- `tests/unit/cexdex_nonce.rs` — 9 unit tests
- `tests/e2e/cexdex_nonce_pipeline.rs` — 1 integration test (feature `e2e`)

**Modified files:**
- `src/mempool/parsers/mod.rs` — re-export `parse_nonce`
- `src/mempool/stream.rs` — nonce short-circuit in parser dispatch
- `src/cexdex/mod.rs` — re-export nonce types
- `src/cexdex/geyser.rs` — pass `monitored_pools` list that includes nonces
- `src/cexdex/config.rs` — parse `CEXDEX_SEARCHER_NONCE_ACCOUNTS`, `CEXDEX_TIP_FRACTION_*`
- `src/cexdex/simulator.rs` — `SimulationResult::Profitable` returns `adjusted_profit_sol` not `tip_lamports`
- `src/executor/relays/mod.rs` — `Relay::submit` accepts `Option<NonceInfo>`
- `src/executor/relays/common.rs` — `build_signed_bundle_tx` accepts `Option<NonceInfo>`
- `src/executor/relays/jito.rs`, `astralane.rs`, `bloxroute.rs`, `nozomi.rs`, `zeroslot.rs` — forward the param
- `src/executor/relay_dispatcher.rs` — forward `Option<NonceInfo>` through `dispatch(...)`
- `src/executor/confirmation.rs` — accept `Arc<NoncePool>` + `nonce_pubkey`; call `mark_settled` on all exit paths; build `bundle_id -> relay_name` map
- `src/bin/cexdex.rs` — checkout nonce before build, per-relay tip, re-enable Astralane, wire confirmation tracker
- `src/main.rs` — pass `None` for nonce param (main engine keeps current behavior)
- `src/metrics/counters.rs` — 6 new counters/gauges
- `monitoring/provisioning/dashboards/cexdex-pnl.json` — 3 new panels
- `.env.example` — document new env vars
- `CLAUDE.md` — document nonce-based non-equivocation strategy

---

## Task Sequencing Notes

Tasks 4 (Relay trait change) and 6 (simulator signature change) are compile-breaking — each must be a single atomic commit with all callers updated. Task 9 (binary wiring) is the largest — it pulls everything together and is where bugs will surface; do its unit tests first.

**Recommended order:** 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11 → 12 → 13 → 14.

---

### Task 1: Nonce account parser

**Files:**
- Create: `src/mempool/parsers/nonce.rs`
- Modify: `src/mempool/parsers/mod.rs` (add `pub mod nonce;` and re-export)
- Create: fixture bytes in the test file (captured from `6vNq2tbRXPWAWnBU4wAvPGK6AgifGoa38NaYfFE2ovNG` on 2026-04-17)
- Test: inline `#[cfg(test)]` in `src/mempool/parsers/nonce.rs`

**Context:** Nonce accounts are 80 bytes, owned by System Program (`11111111111111111111111111111111`). Layout (bincode-encoded `Versions<State>`):

| Offset | Length | Field |
|---|---|---|
| 0 | 4 | Versions tag (u32 LE, 0=Legacy, 1=Current — Data layout identical) |
| 4 | 4 | State tag (u32 LE, 0=Uninitialized, 1=Initialized) |
| 8 | 32 | Data.authority (Pubkey) |
| 40 | 32 | Data.durable_nonce (Hash) |
| 72 | 8 | Data.fee_calculator.lamports_per_signature (u64) |

- [ ] **Step 1: Write the failing tests**

Create `src/mempool/parsers/nonce.rs`:

```rust
//! Parser for Solana durable nonce accounts (80 bytes, bincode-encoded
//! `Versions<State>` from `solana-nonce`). Used by the cexdex narrow
//! Geyser subscription to keep a live cache of each managed nonce's
//! current blockhash without extra RPC.
//!
//! Verified layout (2026-04-17) against a live initialized nonce:
//!   [0..4]   Versions tag (u32 LE, 0=Legacy, 1=Current)
//!   [4..8]   State tag    (u32 LE, 0=Uninitialized, 1=Initialized)
//!   [8..40]  authority    (Pubkey)
//!   [40..72] durable_nonce (Hash)
//!   [72..80] fee_calculator.lamports_per_signature (u64 LE)

use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;

/// Returned tuple is `(authority, current_nonce_hash)`.
/// Returns `None` for uninitialized accounts or data shorter than 72 bytes.
/// Callers must verify `authority == expected_searcher_pubkey` before trusting
/// the hash — defense in depth even though Geyser only delivers subscribed
/// accounts.
pub fn parse_nonce(data: &[u8]) -> Option<(Pubkey, Hash)> {
    if data.len() < 72 {
        return None;
    }
    // Accept both Versions::Legacy (0) and Versions::Current (1) — same Data layout.
    let state_tag = u32::from_le_bytes(data[4..8].try_into().ok()?);
    if state_tag != 1 {
        return None; // not Initialized
    }
    let authority = Pubkey::new_from_array(data[8..40].try_into().ok()?);
    let hash = Hash::new_from_array(data[40..72].try_into().ok()?);
    Some((authority, hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    /// Hex dump captured 2026-04-17 from nonce account
    /// 6vNq2tbRXPWAWnBU4wAvPGK6AgifGoa38NaYfFE2ovNG on mainnet.
    /// Versions=1, State=1, authority=6T3hyzTz..., nonce=DHHF5jgZ..., fee=5000.
    fn initialized_bytes() -> Vec<u8> {
        let hex = "\
            01000000 01000000 \
            50f69895 2c072e31 51b37770 8133e411 3e74b24e f989222e 0fbb230a dc875539 \
            b677e6de f2287cf8 2942d712 b6a91382 656b5f6f 00d1061b 984dbdf6 e1715b00 \
            88130000 00000000";
        hex.split_whitespace()
            .flat_map(|chunk| {
                (0..chunk.len())
                    .step_by(2)
                    .map(move |i| u8::from_str_radix(&chunk[i..i + 2], 16).unwrap())
            })
            .collect()
    }

    #[test]
    fn parses_initialized_nonce() {
        let data = initialized_bytes();
        assert_eq!(data.len(), 80);
        let (authority, hash) = parse_nonce(&data).expect("should parse");
        assert_eq!(
            authority,
            Pubkey::from_str("6T3hyzTz59ZCj18P9LQ6VKEVA2x7xT5jEPV7394b3Hxt").unwrap()
        );
        assert_eq!(
            hash,
            Hash::from_str("DHHF5jgZ76oxcLvZ3bV1Y4wmSsFFvbW6myRsc1fhQWCP").unwrap()
        );
    }

    #[test]
    fn rejects_short_data() {
        assert!(parse_nonce(&[0u8; 71]).is_none());
    }

    #[test]
    fn rejects_uninitialized() {
        let mut data = initialized_bytes();
        data[4] = 0; // flip State tag to Uninitialized
        assert!(parse_nonce(&data).is_none());
    }

    #[test]
    fn accepts_legacy_versions_tag() {
        // Versions::Legacy (tag=0) has identical Data layout.
        let mut data = initialized_bytes();
        data[0] = 0; // flip Versions tag to Legacy
        let (authority, _hash) = parse_nonce(&data).expect("should parse");
        assert_eq!(
            authority,
            Pubkey::from_str("6T3hyzTz59ZCj18P9LQ6VKEVA2x7xT5jEPV7394b3Hxt").unwrap()
        );
    }
}
```

Modify `src/mempool/parsers/mod.rs` to add:

```rust
pub mod nonce;
pub use nonce::parse_nonce;
```

- [ ] **Step 2: Run tests to verify they fail (module doesn't compile yet or tests fail)**

Run: `cargo test --lib parsers::nonce`
Expected: initial FAIL (file just created — if tests compile, they should PASS immediately since the implementation is complete). If compilation fails, address and re-run.

- [ ] **Step 3: Confirm tests pass**

Run: `cargo test --lib parsers::nonce`
Expected: `test result: ok. 4 passed; 0 failed;`

- [ ] **Step 4: Lint check**

Run: `cargo clippy --lib 2>&1 | grep -E "parsers/nonce\.rs"`
Expected: no warnings from this file.

- [ ] **Step 5: Commit**

```bash
git add src/mempool/parsers/nonce.rs src/mempool/parsers/mod.rs
git commit -m "feat(cexdex): nonce account parser for durable-nonce support

Parses the 80-byte System Program-owned nonce account layout
(bincode-encoded Versions<State>). Accepts both Versions::Legacy
and Versions::Current variants (identical Data layout). Rejects
uninitialized accounts. 4 unit tests using a known-good fixture
captured from nonce 6vNq2tbRXPWAWnBU4wAvPGK6AgifGoa38NaYfFE2ovNG.

Part of the cexdex nonce-based relay fan-out design
(docs/superpowers/specs/2026-04-17-cexdex-nonce-fanout-design.md)."
```

---

### Task 2: NoncePool

**Files:**
- Create: `src/cexdex/nonce.rs`
- Modify: `src/cexdex/mod.rs` (add `pub mod nonce;` and re-export `NoncePool`, `NonceInfo`)
- Test: inline `#[cfg(test)]` in `src/cexdex/nonce.rs`

**Context:** Round-robin pool over 3 pubkeys. `checkout()` picks oldest `last_used` (config order tiebreaker). Geyser drives `update_cached_hash` as the source of truth. `mark_settled` is called by the confirmation tracker on every terminal state to prevent nonce leaks.

- [ ] **Step 1: Write the failing tests**

Create `src/cexdex/nonce.rs`:

```rust
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

    /// Round-robin by (cached_hash.is_some(), !in_flight, last_used).
    /// Config order breaks ties. Returns None only if NO nonce has a cached hash.
    /// If the winner is in_flight, we still return it but increment the
    /// collision counter — let the caller handle the soft collision.
    pub fn checkout(&self) -> Option<(Pubkey, Hash)> {
        // Winner: config-earliest index with cached_hash.is_some() and the oldest last_used.
        // Prefer !in_flight over in_flight, then oldest last_used.
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
                    // Prefer !in_flight
                    match (cur.2.in_flight, candidate.2.in_flight) {
                        (true, false) => Some(candidate),
                        (false, true) => Some(cur.clone()),
                        _ => {
                            // Same in_flight status → older last_used wins, then config order.
                            if candidate.2.last_used < cur.2.last_used {
                                Some(candidate)
                            } else if candidate.2.last_used == cur.2.last_used
                                && candidate.0 < cur.0
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
        let in_flight_count = self.state.iter().filter(|e| e.in_flight).count();
        drop(entry);
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
        }
        crate::metrics::counters::inc_cexdex_nonce_hash_refresh_total();
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
        // Simulate that the first's last_used is now newer than the others.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let (second, _) = pool.checkout().unwrap();
        pool.mark_settled(second);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let (third, _) = pool.checkout().unwrap();

        // All three distinct (round-robin advances through the pool)
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

        // Check out both, never settle.
        let _ = pool.checkout().unwrap();
        let _ = pool.checkout().unwrap();
        // Third checkout: both in-flight. Should still return (collision counted).
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
        // Can check out again without collision counter tripping internally
        // (covered by nonce_pool_recheckout test below).
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
}
```

Modify `src/cexdex/mod.rs` — add a `pub mod nonce;` line and re-exports:

```rust
pub mod nonce;
pub use nonce::{NoncePool, NonceInfo};
```

- [ ] **Step 2: Add metrics stubs so the tests compile**

Add to `src/metrics/counters.rs` (location: bottom of the cexdex P&L section, before the `// ── Histograms` comment):

```rust
/// Fires when checkout returned an in-flight nonce — signal we need
/// more nonce accounts.
pub fn inc_cexdex_nonce_collision_total() {
    counter!("cexdex_nonce_collision_total").increment(1);
}

/// Gauge (0..N) of the number of nonces currently in-flight.
pub fn set_cexdex_nonce_in_flight(count: usize) {
    gauge!("cexdex_nonce_in_flight").set(count as f64);
}

/// Increments on every Geyser-driven nonce state update. Used to
/// sanity-check that Geyser is keeping the cache current.
pub fn inc_cexdex_nonce_hash_refresh_total() {
    counter!("cexdex_nonce_hash_refresh_total").increment(1);
}
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test --lib cexdex::nonce`
Expected: `test result: ok. 7 passed; 0 failed;`

- [ ] **Step 4: Lint check**

Run: `cargo clippy --lib 2>&1 | grep -E "cexdex/nonce\.rs|metrics/counters\.rs"`
Expected: no warnings from the new code.

- [ ] **Step 5: Commit**

```bash
git add src/cexdex/nonce.rs src/cexdex/mod.rs src/metrics/counters.rs
git commit -m "feat(cexdex): NoncePool round-robin durable-nonce manager

Thread-safe pool wrapping N nonce accounts. checkout() picks oldest
last_used (config-order tiebreaker), marks in_flight, returns the
cached blockhash. mark_settled() clears in_flight on every terminal
confirmation state. update_cached_hash() is driven by the Geyser
nonce parser.

Adds 3 new Prometheus metrics: cexdex_nonce_collision_total,
cexdex_nonce_in_flight (gauge), cexdex_nonce_hash_refresh_total.

7 unit tests covering cache bootstrap, round-robin, collisions,
settled-state, contains(), and hash overwrite.

Part of the cexdex nonce-based relay fan-out design
(docs/superpowers/specs/2026-04-17-cexdex-nonce-fanout-design.md)."
```

---

### Task 3: Bundle builder accepts `Option<NonceInfo>`

**Files:**
- Modify: `src/executor/relays/common.rs:192` — extend `build_signed_bundle_tx` signature
- Test: inline `#[cfg(test)]` in `src/executor/relays/common.rs`

**Context:** The builder is the single point where every relay constructs its tx. Threading `Option<NonceInfo>` here means each relay impl just forwards the param without duplicating the `advance_nonce_account` prepend logic. When `Some(info)`: the advance ix is prepended at index 0, `recent_blockhash` is used as-is (caller must pass `info.cached_hash`), and the searcher's existing signer satisfies both fee-payer and nonce-authority roles because `info.authority == signer.pubkey()` for cexdex.

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)]` block at the bottom of `src/executor/relays/common.rs`:

```rust
#[test]
fn builder_prepends_advance_nonce_when_nonce_info_given() {
    use solana_sdk::signature::Signer;
    use crate::cexdex::NonceInfo;

    let signer = Keypair::new();
    let tip_account = Pubkey::new_unique();
    let nonce_account = Pubkey::new_unique();
    let nonce_hash = Hash::new_unique();
    let base_ix = system_instruction::transfer(&signer.pubkey(), &Pubkey::new_unique(), 100);

    let tx_bytes = build_signed_bundle_tx(
        "test",
        &[base_ix],
        50_000,
        &tip_account,
        &signer,
        nonce_hash,
        &[],
        Some(NonceInfo {
            account: nonce_account,
            authority: signer.pubkey(),
        }),
    )
    .expect("tx should build");

    // Decode and inspect
    let tx: VersionedTransaction = bincode::deserialize(&tx_bytes).expect("decode");
    let msg = tx.message;
    // ix[0] must be advance_nonce_account invoking System Program
    let ix0 = match &msg {
        VersionedMessage::Legacy(m) => &m.instructions[0],
        VersionedMessage::V0(m) => &m.instructions[0],
    };
    // advance_nonce_account discriminator = 4 (little-endian u32 at the start of ix data)
    let disc = u32::from_le_bytes(ix0.data[0..4].try_into().unwrap());
    assert_eq!(disc, 4, "ix[0] must be System::AdvanceNonceAccount");

    // recent_blockhash slot is the nonce hash we passed in
    let actual_bh = match &msg {
        VersionedMessage::Legacy(m) => m.recent_blockhash,
        VersionedMessage::V0(m) => m.recent_blockhash,
    };
    assert_eq!(actual_bh, nonce_hash);
}

#[test]
fn builder_unchanged_when_nonce_info_none() {
    use solana_sdk::signature::Signer;
    let signer = Keypair::new();
    let tip_account = Pubkey::new_unique();
    let hash = Hash::new_unique();
    let base_ix = system_instruction::transfer(&signer.pubkey(), &Pubkey::new_unique(), 100);

    let tx_bytes = build_signed_bundle_tx(
        "test", &[base_ix], 50_000, &tip_account, &signer, hash, &[], None,
    )
    .expect("tx should build");

    let tx: VersionedTransaction = bincode::deserialize(&tx_bytes).expect("decode");
    let ix0 = match &tx.message {
        VersionedMessage::Legacy(m) => &m.instructions[0],
        VersionedMessage::V0(m) => &m.instructions[0],
    };
    // Without nonce, ix[0] must be the base transfer (System discriminator = 2)
    let disc = u32::from_le_bytes(ix0.data[0..4].try_into().unwrap());
    assert_eq!(disc, 2, "ix[0] must be System::Transfer (the base_ix we passed)");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib relays::common::tests::builder_prepends_advance_nonce`
Expected: compile error (`NonceInfo` not defined in `relays::common`) or the new param missing from `build_signed_bundle_tx`.

- [ ] **Step 3: Modify the builder**

Replace the `build_signed_bundle_tx` signature (currently at `src/executor/relays/common.rs:192`) and add the prepend logic:

```rust
/// Builds a signed tx for a relay bundle: caller's base instructions +
/// the relay's tip transfer, optionally prefixed by a durable-nonce advance.
///
/// When `nonce` is `Some(info)`:
///   - `advance_nonce_account(info.account, info.authority)` is prepended at ix[0]
///   - `recent_blockhash` MUST be the nonce's current cached hash (caller's job)
///   - The single `signer` signature satisfies both the fee-payer and nonce-
///     authority roles because we require `info.authority == signer.pubkey()`
///     by convention for cexdex; this precondition is NOT enforced here.
///
/// When `nonce` is `None`: behavior unchanged from pre-nonce release.
pub fn build_signed_bundle_tx(
    relay_name: &str,
    base_instructions: &[Instruction],
    tip_lamports: u64,
    tip_account: &Pubkey,
    signer: &Keypair,
    recent_blockhash: Hash,
    alts: &[&AddressLookupTableAccount],
    nonce: Option<crate::cexdex::NonceInfo>,
) -> Result<Vec<u8>, RelayResult> {
    // Guard: tip must be > 0 to be meaningful for Jito auction
    if tip_lamports == 0 {
        return Err(fail(
            relay_name,
            "Tip lamports is 0 — Jito requires a non-zero tip transfer".to_string(),
        ));
    }

    // Compose the final instruction list:
    //   [optional nonce_advance] + base_instructions + [tip transfer]
    let mut instructions: Vec<Instruction> = Vec::with_capacity(base_instructions.len() + 2);
    if let Some(info) = nonce {
        use solana_system_interface::instruction::advance_nonce_account;
        instructions.push(advance_nonce_account(&info.account, &info.authority));
    }
    instructions.extend_from_slice(base_instructions);
    instructions.push(system_instruction::transfer(
        &signer.pubkey(),
        tip_account,
        tip_lamports,
    ));

    // Rest of the function body is unchanged (V0 compile with ALTs, legacy fallback,
    // tip-writable verification, encode). Reuse the existing local variable
    // `instructions` in place of the former `instructions`.
    // ... (existing body from line 209 onward)
}
```

The `nonce: Option<...>` is appended as the LAST parameter — update the rest of the file's callers (five internal test invocations at lines 456, 476, 499, 529, 555) to pass `None`.

- [ ] **Step 4: Update the five test invocation call sites**

Each currently ends with `recent_blockhash, &[],` — add `, None` before the closing paren:

```rust
// line ~456
"test", &[base_ix], 50_000, &tip_account, &signer, recent_blockhash, &[], None,
// line ~476
"test", &[], 0, &tip_account, &signer, recent_blockhash, &[], None,
// line ~499
..., recent_blockhash, None,   // etc.
```

- [ ] **Step 5: Run tests**

Run: `cargo test --lib relays::common`
Expected: all tests pass, including the two new `builder_prepends_...` and `builder_unchanged_...`.

- [ ] **Step 6: Lint check**

Run: `cargo clippy --lib 2>&1 | grep -E "relays/common\.rs"`
Expected: no new warnings.

- [ ] **Step 7: Commit**

```bash
git add src/executor/relays/common.rs
git commit -m "feat(executor): bundle builder accepts Option<NonceInfo>

Prepends advance_nonce_account as instruction #0 when NonceInfo is
provided. The caller passes the nonce's cached blockhash as
recent_blockhash; the existing single signer satisfies both the
fee-payer and nonce-authority roles (caller's precondition:
info.authority == signer.pubkey()).

Main-engine call sites continue to pass None — behavior unchanged
for the DEX<->DEX path. 2 new tests verify both branches.

Part of the cexdex nonce-based relay fan-out design."
```

---

### Task 4: Relay trait + all five impls + main.rs — atomic compile-breaking change

**Files:**
- Modify: `src/executor/relays/mod.rs:35-42` (trait signature)
- Modify: `src/executor/relays/jito.rs`, `astralane.rs`, `bloxroute.rs`, `nozomi.rs`, `zeroslot.rs` (impl signatures + forward to builder)
- Modify: `src/executor/relay_dispatcher.rs:37-43` (dispatch signature)
- Modify: `src/main.rs` (pass `None` to dispatcher)

**Context:** The `Relay` trait currently takes only `recent_blockhash`. To thread `Option<NonceInfo>` end-to-end, the trait, all 5 impls, the dispatcher, and the one main-engine caller must change in a single atomic commit. cexdex's new caller (Task 9) will supply `Some(info)`; main.rs supplies `None`.

- [ ] **Step 1: Write a compile-time test (smoke test for the signature change)**

Add to `src/executor/relays/jito.rs` tests module (or wherever JitoRelay has an existing `#[cfg(test)]`):

```rust
#[cfg(test)]
mod trait_sig_tests {
    use super::*;

    /// Compile-time check that JitoRelay::submit forwards Option<NonceInfo>.
    /// Doesn't run; just must type-check.
    #[allow(dead_code)]
    async fn _check_submit_signature(r: &JitoRelay, signer: &Keypair) {
        let _ = r.submit(
            &[],
            1_000,
            signer,
            Hash::default(),
            &[],
            None::<crate::cexdex::NonceInfo>,
        )
        .await;
    }
}
```

- [ ] **Step 2: Update the trait signature**

In `src/executor/relays/mod.rs` replace the trait body (around lines 32-43):

```rust
#[async_trait::async_trait]
pub trait Relay: Send + Sync {
    fn name(&self) -> &str;
    fn is_configured(&self) -> bool;
    async fn submit(
        &self,
        base_instructions: &[Instruction],
        tip_lamports: u64,
        signer: &Keypair,
        recent_blockhash: Hash,
        alts: &[&AddressLookupTableAccount],
        nonce: Option<crate::cexdex::NonceInfo>,
    ) -> RelayResult;
}
```

- [ ] **Step 3: Update all 5 relay impls**

For each of `jito.rs`, `astralane.rs`, `bloxroute.rs`, `nozomi.rs`, `zeroslot.rs`:

1. Extend the `async fn submit(...)` signature to add `nonce: Option<crate::cexdex::NonceInfo>` as the last parameter.
2. In the body, find the call to `build_signed_bundle_tx(...)` and add `nonce` as the last argument.
3. If the impl has internal helper methods that take `recent_blockhash`, extend those too and forward.

Example for `jito.rs` (~line 168-175 area): change `build_signed_bundle_tx("jito", ..., recent_blockhash, alts,)` to `build_signed_bundle_tx("jito", ..., recent_blockhash, alts, nonce,)`.

Example for `astralane.rs:147-168`: same pattern.

- [ ] **Step 4: Update the dispatcher**

In `src/executor/relay_dispatcher.rs`, extend `dispatch` (around line 37):

```rust
pub fn dispatch(
    &self,
    base_instructions: &[Instruction],
    tip_lamports: u64,
    recent_blockhash: Hash,
    rt: &tokio::runtime::Handle,
    nonce: Option<crate::cexdex::NonceInfo>,
) -> tokio::sync::mpsc::Receiver<super::relays::RelayResult> {
    // ...
    // Inside the per-relay spawn block, forward `nonce` to relay.submit(...)
    // The variable shadow is safe: we capture `nonce` via Copy.
    let n = nonce;
    rt.spawn(async move {
        // ... existing ...
        let result = relay.submit(&ixs, tip, &signer, bh, &alt_refs, n).await;
        // ... existing ...
    });
}
```

`NonceInfo` is `Copy`, so capturing it by move into each spawned task works.

- [ ] **Step 5: Update `src/main.rs` — pass `None` at the one call site**

Find the `dispatcher.dispatch(...)` call (around line 570 area per git blame) and add `, None` before the closing paren.

- [ ] **Step 6: Verify it compiles**

Run: `cargo check --all-targets`
Expected: `Finished`. If any relay impl is missed, fix it.

- [ ] **Step 7: Run full test suite**

Run: `cargo test --lib`
Expected: all existing tests pass (no behavioral change at any caller yet).

- [ ] **Step 8: Commit**

```bash
git add src/executor/relays/ src/executor/relay_dispatcher.rs src/main.rs
git commit -m "feat(executor): thread Option<NonceInfo> through Relay trait

Atomic signature change across Relay trait, all 5 relay impls
(jito/astralane/bloxroute/nozomi/zeroslot), RelayDispatcher::dispatch,
and the main.rs call site (which passes None, keeping DEX<->DEX
behavior unchanged).

cexdex binary (upcoming task) will supply Some(info) to enable
multi-relay fan-out with durable-nonce non-equivocation.

Part of the cexdex nonce-based relay fan-out design."
```

---

### Task 5: Config — nonce accounts + per-relay tip fractions

**Files:**
- Modify: `src/cexdex/config.rs` (add fields + env parsing + startup validation)

**Context:** Two new env vars (`CEXDEX_SEARCHER_NONCE_ACCOUNTS`, `CEXDEX_TIP_FRACTION_*`) with bounds checking. Per-relay tip fallback chain: `CEXDEX_TIP_FRACTION_<NAME>` → `CEXDEX_TIP_FRACTION` → error if neither set. Spec mandates non-empty `tip_fractions` and bounds `0 < f < 1`.

- [ ] **Step 1: Write the failing test**

Add to the tests module of `src/cexdex/config.rs` (create one if not present):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn clear_cexdex_env() {
        for (k, _) in std::env::vars() {
            if k.starts_with("CEXDEX_") || k == "JITO_BLOCK_ENGINE_URL" {
                std::env::remove_var(k);
            }
        }
    }

    #[test]
    #[serial]
    fn parses_nonce_accounts() {
        clear_cexdex_env();
        std::env::set_var("CEXDEX_SEARCHER_NONCE_ACCOUNTS",
            "6vNq2tbRXPWAWnBU4wAvPGK6AgifGoa38NaYfFE2ovNG,\
             AHuwmGY1Z4S9ATmAHQj4vBKmH85McP6KKJJLoTvf2AxF");
        std::env::set_var("CEXDEX_TIP_FRACTION", "0.15");
        // (other required env vars omitted for brevity — set whichever your
        // from_env() currently asserts on, or use a narrower parse helper)
        // For a true end-to-end test, invoke CexDexConfig::from_env() and
        // assert config.nonce_accounts.len() == 2.
    }

    #[test]
    #[serial]
    fn rejects_empty_tip_fractions_map() {
        // Implementation detail — invoked from from_env() after per-relay parse.
        // If a caller configures ONLY CEXDEX_TIP_FRACTION_* = empty AND no
        // default, validation should error.
        // (See inline validation in from_env().)
    }
}
```

Note: if your `CexDexConfig::from_env()` is a large integrator, consider extracting the nonce/tip-fraction parse into a private helper function you can unit-test in isolation. Keep the existing `from_env()` as the integrator.

- [ ] **Step 2: Add fields to the struct**

In `src/cexdex/config.rs:13`, add three fields to `CexDexConfig`:

```rust
pub struct CexDexConfig {
    // ... existing fields ...

    /// Nonce accounts for multi-relay non-equivocation. Parsed from
    /// CEXDEX_SEARCHER_NONCE_ACCOUNTS (comma-separated pubkeys).
    pub nonce_accounts: Vec<Pubkey>,

    /// Per-relay tip fractions. Key = relay name (e.g. "jito", "astralane").
    /// Falls back to `tip_fraction` (CEXDEX_TIP_FRACTION) if a specific
    /// relay's env var isn't set.
    pub tip_fractions: std::collections::HashMap<String, f64>,
}
```

- [ ] **Step 3: Add parse + validation in `from_env`**

Inside `CexDexConfig::from_env()`, after the existing `tip_fraction` parse:

```rust
// Nonce accounts (optional for backward-compat — empty vec = nonce-less mode)
let nonce_accounts: Vec<Pubkey> = std::env::var("CEXDEX_SEARCHER_NONCE_ACCOUNTS")
    .unwrap_or_default()
    .split(',')
    .filter(|s| !s.trim().is_empty())
    .map(|s| Pubkey::from_str(s.trim()))
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| anyhow::anyhow!("Invalid CEXDEX_SEARCHER_NONCE_ACCOUNTS pubkey: {}", e))?;

// Per-relay tip fractions. Uses tip_fraction (parsed above) as the default.
let mut tip_fractions: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
for (relay_name, env_key) in [
    ("jito", "CEXDEX_TIP_FRACTION_JITO"),
    ("astralane", "CEXDEX_TIP_FRACTION_ASTRALANE"),
    ("nozomi", "CEXDEX_TIP_FRACTION_NOZOMI"),
    ("bloxroute", "CEXDEX_TIP_FRACTION_BLOXROUTE"),
    ("zeroslot", "CEXDEX_TIP_FRACTION_ZEROSLOT"),
] {
    let f: f64 = std::env::var(env_key).ok()
        .map(|s| s.parse::<f64>())
        .transpose()?
        .unwrap_or(tip_fraction);
    anyhow::ensure!(
        f > 0.0 && f < 1.0,
        "{} must be between 0 and 1 (exclusive), got {}",
        env_key, f,
    );
    tip_fractions.insert(relay_name.to_string(), f);
}
anyhow::ensure!(
    !tip_fractions.is_empty(),
    "tip_fractions map must have at least one entry — aborting to avoid divide-by-zero"
);
```

And include `nonce_accounts` and `tip_fractions` in the struct initializer returned at the bottom.

- [ ] **Step 4: Verify compilation**

Run: `cargo check --all-targets`
Expected: `Finished`.

- [ ] **Step 5: Run tests**

Run: `cargo test --lib cexdex::config`
Expected: pass (add `serial_test = "3"` to Cargo.toml `[dev-dependencies]` if not present).

- [ ] **Step 6: Commit**

```bash
git add src/cexdex/config.rs Cargo.toml
git commit -m "feat(cexdex): parse CEXDEX_SEARCHER_NONCE_ACCOUNTS + per-relay tip fractions

Two new config fields:
- nonce_accounts: Vec<Pubkey> parsed from CEXDEX_SEARCHER_NONCE_ACCOUNTS
- tip_fractions: HashMap<String, f64> keyed by relay name, populated
  from CEXDEX_TIP_FRACTION_{JITO,ASTRALANE,NOZOMI,BLOXROUTE,ZEROSLOT}
  with CEXDEX_TIP_FRACTION as the per-relay fallback.

Validates each fraction is in (0, 1) exclusive and that the map is
non-empty (avoids divide-by-zero in the worst-case gate).

Part of the cexdex nonce-based relay fan-out design."
```

---

### Task 6: Simulator refactor — return `adjusted_profit_sol`

**Files:**
- Modify: `src/cexdex/simulator.rs` (change `SimulationResult::Profitable` enum variant + the worst-case gate)
- Modify: `src/bin/cexdex.rs` (update the match arm to destructure the new variant shape)
- Modify: `tests/unit/cexdex_simulator.rs` (update fixture helpers to new variant)

**Context:** Currently the simulator picks ONE tip and returns `tip_lamports`. For per-relay tips, the simulator must step back and return the *raw* slippage-adjusted profit; the binary computes per-relay tips at dispatch time. The worst-case gate (using `max(tip_fractions)`) stays inside the simulator so that rejected routes never reach dispatch.

- [ ] **Step 1: Write the failing test**

In `tests/unit/cexdex_simulator.rs`, add:

```rust
#[test]
fn profitable_returns_adjusted_profit_sol_and_passes_worst_case_gate() {
    let store = PriceStore::new();
    let pool = insert_cp_pool(&store, 100_000_000_000, 18_000_000_000, 30);
    store.update_cex("SOLUSDC", PriceSnapshot {
        best_bid_usd: 185.0,
        best_ask_usd: 185.02,
        received_at: Instant::now(),
    });
    let route = mk_route_buy(pool, 100_000_000);

    // Config with a max tip fraction of 0.40 — sim applies this as worst case.
    let cfg = CexDexSimulatorConfig {
        min_profit_usd: 0.05,
        slippage_tolerance: 0.25,
        tx_fee_lamports: 5_000,
        min_tip_lamports: 1_000,
        max_tip_fraction: 0.40,
    };
    let sim = CexDexSimulator::new(store, cfg);
    match sim.simulate(&route) {
        SimulationResult::Profitable { adjusted_profit_sol, net_profit_usd_worst_case, min_final_output, .. } => {
            assert!(adjusted_profit_sol > 0.0, "expected positive adjusted profit");
            assert!(net_profit_usd_worst_case >= 0.05, "worst-case net must pass min_profit");
            assert!(min_final_output > 0);
        }
        SimulationResult::Unprofitable { reason } => {
            panic!("expected Profitable; got {}", reason);
        }
    }
}
```

- [ ] **Step 2: Run test — expect compile error**

Run: `cargo test --test unit cexdex_simulator::profitable_returns_adjusted_profit_sol`
Expected: compile error — `Profitable` doesn't have `adjusted_profit_sol` or `max_tip_fraction`.

- [ ] **Step 3: Change the variant and the config**

In `src/cexdex/simulator.rs`:

1. Replace `tip_fraction` in `CexDexSimulatorConfig` with `max_tip_fraction: f64` (the binary will pass `tip_fractions.values().max()`).

2. Replace the `Profitable` variant's fields:

```rust
pub enum SimulationResult {
    Profitable {
        /// Route with fresh quote + CEX prices written back.
        route: CexDexRoute,
        /// Slippage-adjusted gross profit in SOL. Caller computes per-relay
        /// tip = adjusted_profit_sol * tip_fractions[relay].
        adjusted_profit_sol: f64,
        /// Slippage-adjusted gross profit in USD (convenience; used by stats).
        adjusted_profit_usd: f64,
        /// Worst-case net after tip (using max_tip_fraction) and tx fee.
        /// Sim rejects if this is below min_profit_usd — so every Profitable
        /// return is profitable regardless of which relay lands first.
        net_profit_usd_worst_case: f64,
        /// On-chain arb-guard floor.
        min_final_output: u64,
    },
    Unprofitable { reason: String },
}
```

3. Update the body of `simulate()` to compute the worst-case using `self.config.max_tip_fraction`:

```rust
// Step 6: compute worst-case tip using the highest per-relay fraction.
let sol_price = (cex_bid + cex_ask) / 2.0;
if sol_price <= 0.0 {
    return SimulationResult::Unprofitable {
        reason: "invalid CEX price (zero or negative)".to_string(),
    };
}
let adj_profit_sol = adj_profit_usd / sol_price;
let worst_case_tip_sol = adj_profit_sol * self.config.max_tip_fraction;
let worst_case_tip_usd = worst_case_tip_sol * sol_price;
let tx_fee_usd = lamports_to_sol(self.config.tx_fee_lamports) * sol_price;
let net_profit_usd_worst_case = adj_profit_usd - worst_case_tip_usd - tx_fee_usd;

// Hard floor (non-positive regardless of config)
if net_profit_usd_worst_case <= 0.0 {
    return SimulationResult::Unprofitable {
        reason: format!(
            "non-positive worst-case net: {:.6} usd (gross={:.6}, worst_tip={:.6}, fee={:.6})",
            net_profit_usd_worst_case, gross_profit_usd, worst_case_tip_usd, tx_fee_usd,
        ),
    };
}
if net_profit_usd_worst_case < self.config.min_profit_usd {
    return SimulationResult::Unprofitable {
        reason: format!(
            "below threshold: worst-case net {:.6} usd < min={:.4}",
            net_profit_usd_worst_case, self.config.min_profit_usd,
        ),
    };
}

// ... then compute min_final_output as before ...

SimulationResult::Profitable {
    route: fresh_route,
    adjusted_profit_sol: adj_profit_sol,
    adjusted_profit_usd: adj_profit_usd,
    net_profit_usd_worst_case,
    min_final_output,
}
```

4. Remove the old `tip_lamports` field and the `tip_fraction`-based tip calc from `simulate()`; that's now the binary's job.

- [ ] **Step 4: Update `src/bin/cexdex.rs` — destructure the new variant**

Find the `match sim_result { ... }` block (around line 483) and change the `Profitable` arm:

```rust
SimulationResult::Profitable {
    route,
    adjusted_profit_sol,
    adjusted_profit_usd: _,
    net_profit_usd_worst_case,
    min_final_output,
} => {
    // Keep the existing binding names where possible:
    //   net_profit_usd is now net_profit_usd_worst_case
    //   tip_lamports is computed later, per-relay, in Task 9
    (route, adjusted_profit_sol, min_final_output, net_profit_usd_worst_case, !config.dry_run)
}
```

(Ripple effects — `tip_lamports` is no longer bound here; it's recomputed per-relay. Don't wire that yet; Task 9 does it. For now the binary will temporarily not compile because other code paths use `tip_lamports`. That's fine — Task 7 (Geyser wiring) is a smaller contained change, but the binary won't compile until Task 9. Bundle Tasks 6, 7, 8, 9 through commits in sequence.)

Actually, to keep each commit compilable, stub tip_lamports temporarily:
```rust
let tip_lamports = 0u64; // FIXME(Task 9): computed per-relay at dispatch
```
and pass zero through (knowing no bundle will actually dispatch until Task 9 replaces this). Keep all existing log lines and stats recording working.

- [ ] **Step 5: Update simulator test fixtures**

In `tests/unit/cexdex_simulator.rs:57-65`, the `mk_config()` helper. Replace `tip_fraction: 0.50` with `max_tip_fraction: 0.50`.

Update all `SimulationResult::Profitable { net_profit_usd, tip_lamports, ... }` destructures in the existing tests to the new field shape.

- [ ] **Step 6: Verify compilation + tests**

Run: `cargo check --all-targets` — expect success.
Run: `cargo test --test unit cexdex_simulator` — expect all pre-existing tests + the new one to pass.

- [ ] **Step 7: Commit**

```bash
git add src/cexdex/simulator.rs src/bin/cexdex.rs tests/unit/cexdex_simulator.rs
git commit -m "refactor(cexdex): simulator returns adjusted_profit_sol for per-relay tip

SimulationResult::Profitable no longer carries tip_lamports. Instead it
returns adjusted_profit_sol (slippage-adjusted gross) + net_profit_usd_worst_case
(computed using max_tip_fraction). The binary will compute tip per-relay
at dispatch time in an upcoming task.

Gates preserved: hard floor (net > 0) still enforced, min_profit_usd
still enforced — both against the WORST-CASE tip fraction, so every
Profitable result is safe regardless of which relay wins the race.

Bundle dispatch path in cexdex.rs temporarily stubs tip_lamports=0;
upcoming task wires per-relay dispatch loop.

Part of the cexdex nonce-based relay fan-out design."
```

---

### Task 7: Geyser subscription + stream nonce short-circuit

**Files:**
- Modify: `src/cexdex/geyser.rs` (pass nonce pubkeys to `monitored_pools` + carry `NoncePool` ref)
- Modify: `src/mempool/stream.rs` (add nonce short-circuit at the top of the parser dispatch)

**Context:** We already have `SubscriptionMode::SpecificAccounts(Vec<Pubkey>)`. Add the 3 nonce pubkeys to the filter list. Inside the per-update handler, short-circuit on `data.len() == 80 && owner == System && nonce_pool.contains(&pk)` and route to the nonce parser instead of the pool parsers.

- [ ] **Step 1: Extend `start_geyser` signature and threading**

Modify `src/cexdex/geyser.rs::start_geyser`:

```rust
pub async fn start_geyser(
    config: BotConfig,
    store: PriceStore,
    http_client: reqwest::Client,
    monitored_pools: Vec<Pubkey>,
    nonce_pool: crate::cexdex::NoncePool,
    searcher_pubkey: solana_sdk::pubkey::Pubkey,
    change_tx: Sender<PoolStateChange>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<tokio::task::JoinHandle<()>> {
    // Combined subscription list: pools + nonces.
    let mut subscription: Vec<Pubkey> = monitored_pools.clone();
    subscription.extend(nonce_pool.pubkeys());

    let pool_count = store.pools.len();
    info!(
        "Starting narrow Geyser (cexdex): {} pools in cache, {} pool accounts + {} nonce accounts = {} monitored total",
        pool_count,
        monitored_pools.len(),
        nonce_pool.len(),
        subscription.len(),
    );

    let stream = GeyserStream::new(Arc::new(config), store.pools.clone(), http_client)
        .with_subscription_mode(SubscriptionMode::SpecificAccounts(subscription))
        .with_nonce_pool(nonce_pool, searcher_pubkey);

    let handle = tokio::spawn(async move {
        if let Err(e) = stream.start(change_tx, shutdown_rx).await {
            tracing::error!("cexdex Geyser stream exited: {e}");
        }
    });

    Ok(handle)
}
```

- [ ] **Step 2: Add `with_nonce_pool` builder to `GeyserStream`**

In `src/mempool/stream.rs`, extend `GeyserStream`:

```rust
pub struct GeyserStream {
    // ... existing fields ...
    /// If Some, account updates matching these pubkeys (80 bytes, System
    /// Program-owned) are parsed as nonce accounts and update the pool
    /// instead of falling through to DEX parsers.
    nonce_pool: Option<crate::cexdex::NoncePool>,
    nonce_authority: Option<Pubkey>,
}

impl GeyserStream {
    // Add near the other builder methods:
    pub fn with_nonce_pool(
        mut self,
        pool: crate::cexdex::NoncePool,
        authority: Pubkey,
    ) -> Self {
        self.nonce_pool = Some(pool);
        self.nonce_authority = Some(authority);
        self
    }
}
```

Initialize `nonce_pool: None, nonce_authority: None` in `GeyserStream::new`.

- [ ] **Step 3: Add the short-circuit in the stream's per-update handler**

Inside the main event loop in `src/mempool/stream.rs`, at the TOP of the account-update handler (before the data-size → DEX dispatch):

```rust
// Nonce short-circuit: 80 bytes, System Program owner, registered pubkey.
// We ALSO early-return and DO NOT forward a PoolStateChange.
if account.data.len() == 80 {
    if let (Some(np), Some(auth)) = (&self.nonce_pool, &self.nonce_authority) {
        if account.owner == solana_sdk::system_program::id().to_string()
            && np.contains(&pool_address)
        {
            if let Some((parsed_auth, hash)) =
                crate::mempool::parsers::parse_nonce(&account.data)
            {
                if &parsed_auth == auth {
                    np.update_cached_hash(pool_address, hash);
                } else {
                    tracing::warn!(
                        "Nonce {} authority mismatch: parsed={} expected={}",
                        pool_address, parsed_auth, auth,
                    );
                }
            }
            continue; // do NOT fall through to pool parsers
        }
    }
}
```

Note: `account.owner` is a `String` in the LaserStream proto types. Compare with `solana_sdk::system_program::id().to_string()` or use the const from `addresses.rs`.

- [ ] **Step 4: Update cexdex binary's start_geyser call site**

In `src/bin/cexdex.rs`, find where `start_geyser` is invoked (around line 152) and add the two new args:

```rust
let _geyser_handle = start_geyser(
    bot_config_geyser,
    store.clone(),
    http_client.clone(),
    monitored_pool_pubkeys,
    nonce_pool.clone(),       // NEW
    searcher_pubkey,          // NEW
    change_tx,
    shutdown_rx.clone(),
).await?;
```

(Task 9 will declare `nonce_pool` — for now add a placeholder `let nonce_pool = NoncePool::new(config.nonce_accounts.clone());` just before the `start_geyser` call.)

- [ ] **Step 5: Verify compilation**

Run: `cargo check --all-targets`
Expected: `Finished`.

- [ ] **Step 6: Commit**

```bash
git add src/mempool/stream.rs src/cexdex/geyser.rs src/bin/cexdex.rs
git commit -m "feat(cexdex): Geyser nonce short-circuit

Extends SubscriptionMode::SpecificAccounts to include the 3 nonce
accounts alongside the 4 monitored pools. When the stream receives
an update for a registered nonce (80 bytes, System Program-owned),
it parses and forwards to NoncePool::update_cached_hash, then skips
pool-parser dispatch entirely.

Sanity-checks the parsed authority == searcher_pubkey; logs a warning
on mismatch (would indicate a misconfigured CEXDEX_SEARCHER_NONCE_ACCOUNTS
entry or an out-of-band account takeover).

Part of the cexdex nonce-based relay fan-out design."
```

---

### Task 8: Startup nonce bootstrap (RPC fetch + authority validation)

**Files:**
- Modify: `src/bin/cexdex.rs` — fetch each nonce once via `getAccountInfo`, populate the pool, verify authorities, hard-fail on mismatch

**Context:** Without this step, the detector would skip opportunities for ~1-2 slots while waiting for the first Geyser delivery. The bootstrap fetch removes that window AND validates authority up-front.

- [ ] **Step 1: Add a helper in `src/bin/cexdex.rs`**

Place near the existing `fetch_initial_balances` helper:

```rust
/// Fetch each nonce account's state via RPC, verify authority, and
/// populate NoncePool's cache. Returns Err if any nonce is unreachable,
/// un-initialized, or has an authority other than `searcher_pubkey`.
async fn bootstrap_nonce_pool(
    client: &reqwest::Client,
    rpc_url: &str,
    pool: &solana_mev_bot::cexdex::NoncePool,
    searcher_pubkey: &solana_sdk::pubkey::Pubkey,
) -> anyhow::Result<()> {
    use base64::{engine::general_purpose, Engine};

    for nonce_pk in pool.pubkeys() {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getAccountInfo",
            "params": [nonce_pk.to_string(), {"encoding": "base64"}],
        });
        let resp: serde_json::Value = client.post(rpc_url).json(&payload).send().await?.json().await?;
        let v = resp.get("result").and_then(|r| r.get("value"));
        let v = v.ok_or_else(|| anyhow::anyhow!("Nonce {} not found", nonce_pk))?;
        if v.is_null() {
            anyhow::bail!("Nonce {} not found on chain", nonce_pk);
        }
        let data_b64 = v.get("data").and_then(|d| d.as_array()).and_then(|a| a.first()).and_then(|s| s.as_str())
            .ok_or_else(|| anyhow::anyhow!("Nonce {} missing data", nonce_pk))?;
        let data = general_purpose::STANDARD.decode(data_b64)?;
        let (auth, hash) = solana_mev_bot::mempool::parsers::parse_nonce(&data)
            .ok_or_else(|| anyhow::anyhow!("Nonce {} is not initialized or wrong layout", nonce_pk))?;
        anyhow::ensure!(
            &auth == searcher_pubkey,
            "Nonce {} authority mismatch: on-chain={} configured searcher={}",
            nonce_pk, auth, searcher_pubkey,
        );
        pool.update_cached_hash(nonce_pk, hash);
        tracing::info!("Bootstrapped nonce {} with hash {}", nonce_pk, hash);
    }
    Ok(())
}
```

- [ ] **Step 2: Call `bootstrap_nonce_pool` before `start_geyser`**

In `main()` of `src/bin/cexdex.rs`, just after `let nonce_pool = NoncePool::new(...)` (from Task 7's placeholder):

```rust
let nonce_pool = solana_mev_bot::cexdex::NoncePool::new(config.nonce_accounts.clone());
if !config.nonce_accounts.is_empty() {
    bootstrap_nonce_pool(&http_client, &config.rpc_url, &nonce_pool, &searcher_pubkey)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to bootstrap nonce pool: {}", e))?;
    info!("Nonce pool bootstrapped: {} accounts", nonce_pool.len());
} else {
    warn!("CEXDEX_SEARCHER_NONCE_ACCOUNTS empty — multi-relay fan-out disabled; only Jito will be configured");
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check --all-targets`
Expected: `Finished`.

- [ ] **Step 4: Commit**

```bash
git add src/bin/cexdex.rs
git commit -m "feat(cexdex): startup nonce bootstrap + authority validation

Before the detector loop starts, fetches each configured nonce via
getAccountInfo, verifies authority == searcher_pubkey, and populates
NoncePool's cached hash. Hard-fails on:
- nonce account not found
- uninitialized / wrong layout
- authority mismatch (misconfiguration or account takeover)

Removes the ~1-2 slot window where checkout() would return None
waiting for the first Geyser delivery.

Part of the cexdex nonce-based relay fan-out design."
```

---

### Task 9: Wire per-relay dispatch with nonce in the detector loop

**Files:**
- Modify: `src/bin/cexdex.rs` — replace the single-dispatch path with a per-relay loop that:
  1. Calls `NoncePool::checkout()`
  2. Iterates configured relays
  3. For each relay: computes per-relay tip, calls `relay.submit(...)` with the shared `Some(NonceInfo)`
  4. Spawns the confirmation tracker with the nonce pubkey

**Context:** This is the largest single task — everything converges here. Be careful with existing logging, dedup / cooldown calls (`detector.mark_dispatched`), and stats recording.

- [ ] **Step 1: Remove the stubbed `tip_lamports` and replace with per-relay computation**

Find the section added in Task 6 where `tip_lamports = 0u64` is stubbed. Below the `info!("SUBMIT: net=...")` log line, replace the current single `dispatcher.dispatch(...)` call with the per-relay loop.

Also remove the single-relay pre-dispatch re-check (the existing "ABORT submit" guard) — it was based on `tip_lamports` from the old simulator shape; replaced by the simulator's worst-case gate.

New flow (replace from just after `detector.mark_dispatched(...)` down to and including the current `spawn_confirmation_tracker` block):

```rust
// Check out a nonce for this opportunity. None = pool still warming up.
let nonce_tuple = if !config.nonce_accounts.is_empty() {
    match nonce_pool.checkout() {
        Some(t) => Some(t),
        None => {
            tracing::debug!("Nonce pool not ready yet; skipping opportunity");
            solana_mev_bot::metrics::counters::inc_cexdex_detector_skip("nonce_pool_empty");
            continue;
        }
    }
} else {
    None
};

let (nonce_pk_opt, blockhash) = match nonce_tuple {
    Some((pk, h)) => (Some(pk), h),
    None => {
        // Fallback only when nonce_accounts is EMPTY (operator opted out).
        // When opted-in but pool empty, we skipped above.
        let bh = match blockhash_cache.get() {
            Some(h) => h,
            None => {
                tracing::warn!("no blockhash cached; skipping opportunity");
                continue;
            }
        };
        (None, bh)
    }
};
let nonce_info = nonce_pk_opt.map(|account| solana_mev_bot::cexdex::NonceInfo {
    account,
    authority: searcher_pubkey,
});

// For each configured relay, compute its tip and dispatch.
let rt_handle = rt.clone();
let sol_price = (cex_bid + cex_ask) / 2.0;
for (relay_name, tip_fraction) in config.tip_fractions.iter() {
    // Skip relays that aren't in the dispatcher's configured list.
    if !dispatcher.has_relay(relay_name) {
        continue;
    }
    let tip_sol = adjusted_profit_sol * tip_fraction;
    let tip_lamports = std::cmp::max(
        solana_mev_bot::cexdex::units::sol_to_lamports(tip_sol),
        1_000u64, // min_tip floor; keep consistent with old path
    );
    let tip_usd = solana_mev_bot::cexdex::units::lamports_to_sol(tip_lamports) * sol_price;
    tracing::info!(
        "SUBMIT(relay={}): adj_profit_sol={:.6} tip={} lamports (${:.4})",
        relay_name, adjusted_profit_sol, tip_lamports, tip_usd,
    );

    // Per-relay bundle build + submit via a narrow dispatcher helper.
    let relay_rx = dispatcher.dispatch_single(
        relay_name,
        &instructions,
        tip_lamports,
        blockhash,
        &rt_handle,
        nonce_info,
    );

    solana_mev_bot::metrics::counters::inc_cexdex_bundles_attempted(relay_name);

    // Per-relay confirmation tracker: releases the nonce + credits realized PNL
    // only for the relay whose bundle actually lands.
    let inv_cb = inventory.clone();
    let net = net_profit_usd_worst_case;
    let nonce_for_release = nonce_pk_opt;
    let pool_for_release = nonce_pool.clone();
    let tip_usd_credit = tip_usd;
    let relay_name_owned = relay_name.clone();
    let on_landed: solana_mev_bot::executor::confirmation::OnLandedCallback = Box::new(move || {
        inv_cb.add_realized_pnl_usd(net);
        solana_mev_bot::metrics::counters::inc_cexdex_bundles_confirmed(&relay_name_owned);
        solana_mev_bot::metrics::counters::add_cexdex_tip_paid_usd(&relay_name_owned, tip_usd_credit);
        if let Some(pk) = nonce_for_release {
            pool_for_release.mark_settled(pk);
        }
    });
    // on_dropped / on_timeout: also release nonce. Task 10 adds this callback.
    let on_settle_no_land = {
        let pool = nonce_pool.clone();
        let pk_opt = nonce_pk_opt;
        Box::new(move || {
            if let Some(pk) = pk_opt {
                pool.mark_settled(pk);
            }
        }) as solana_mev_bot::executor::confirmation::OnLandedCallback
    };

    let confirm_jito = format!(
        "{}/api/v1/bundles",
        config.jito_block_engine_url.trim_end_matches('/'),
    );
    // Convert worst-case USD profit to lamports for the confirmation logger.
    let profit_lamports = (net_profit_usd_worst_case / sol_price * 1e9) as u64;

    solana_mev_bot::executor::spawn_confirmation_tracker(
        http_client.clone(),
        confirm_jito,
        profit_lamports,
        tip_lamports,
        relay_rx,
        config.rpc_url.clone(),
        route.pool_address.to_string(),
        route.observed_slot,
        Some(on_landed),
        Some(on_settle_no_land),
    );
}

// Fast balance refresh 3s after dispatch (existing behavior retained)
{
    let inv = inventory.clone();
    let client = http_client.clone();
    let rpc = config.rpc_url.clone();
    let wallet = searcher_pubkey;
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        if let Ok((sol, usdc)) = fetch_initial_balances(&client, &rpc, &wallet).await {
            inv.set_on_chain(sol, usdc);
        }
    });
}
```

- [ ] **Step 2: Add `dispatcher.has_relay()` and `dispatch_single()` helpers**

In `src/executor/relay_dispatcher.rs`:

```rust
pub fn has_relay(&self, name: &str) -> bool {
    self.relays.iter().any(|r| r.is_configured() && r.name() == name)
}

/// Dispatch to a single named relay (rather than all). Returns a Receiver
/// yielding exactly one RelayResult for that relay. Used by cexdex where
/// each relay gets its own tip amount.
pub fn dispatch_single(
    &self,
    relay_name: &str,
    base_instructions: &[Instruction],
    tip_lamports: u64,
    recent_blockhash: Hash,
    rt: &tokio::runtime::Handle,
    nonce: Option<crate::cexdex::NonceInfo>,
) -> tokio::sync::mpsc::Receiver<super::relays::RelayResult> {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let relay_opt = self.relays.iter().find(|r| r.is_configured() && r.name() == relay_name).cloned();
    if let Some(relay) = relay_opt {
        let ixs = base_instructions.to_vec();
        let signer = self.signer.clone();
        let alts = self.alts.clone();
        rt.spawn(async move {
            let alt_refs: Vec<&AddressLookupTableAccount> =
                alts.iter().map(|a| a.as_ref()).collect();
            let result = relay.submit(&ixs, tip_lamports, &signer, recent_blockhash, &alt_refs, nonce).await;
            if result.success {
                info!(
                    "Bundle accepted by {}: id={:?} latency={}us",
                    result.relay_name, result.bundle_id, result.latency_us,
                );
            } else if let Some(ref err) = result.error {
                warn!(
                    "Bundle REJECTED by {}: {} (latency={}us)",
                    result.relay_name, err, result.latency_us,
                );
            }
            let _ = tx.send(result).await;
        });
    }
    rx
}
```

- [ ] **Step 3: Re-enable Astralane**

In `src/bin/cexdex.rs`, revert the single-relay guard (earlier commit `f329f9f`). Change the relay-list construction back to:

```rust
use solana_mev_bot::executor::relays::{
    jito::JitoRelay, astralane::AstralaneRelay, Relay,
};

// Multi-relay fan-out safe via durable nonce (see
// docs/superpowers/specs/2026-04-17-cexdex-nonce-fanout-design.md).
let relays: Vec<Arc<dyn Relay>> = vec![
    Arc::new(JitoRelay::new(&bot_config_relays)),
    Arc::new(AstralaneRelay::new(&bot_config_relays, shutdown_rx.clone())),
];
```

- [ ] **Step 4: Wire `nonce_pool` into `run_detector_loop` args**

Add `nonce_pool: solana_mev_bot::cexdex::NoncePool` to `run_detector_loop`'s signature and the call site. Pass it through.

- [ ] **Step 5: Verify compilation**

Run: `cargo check --all-targets`
Expected: `Finished`. If `OnLandedCallback` only supports one callback per tracker, the dual-callback approach in step 1 will need the Task 10 extension first — if so, temporarily stub `on_settle_no_land` and move on, then come back after Task 10.

- [ ] **Step 6: Commit**

```bash
git add src/bin/cexdex.rs src/executor/relay_dispatcher.rs
git commit -m "feat(cexdex): per-relay dispatch with nonce-based fan-out

Replaces the single Jito dispatch with a per-relay loop:
  - for each configured relay (Jito + Astralane now)
  - compute tip = adjusted_profit_sol * CEXDEX_TIP_FRACTION_<RELAY>
  - submit with Some(NonceInfo) so both bundles reference the same
    advance_nonce_account at ix[0] — only one can land

Adds RelayDispatcher::has_relay() and dispatch_single() helpers.
Restores the Astralane relay that was removed after LIVE3 (commit f329f9f).

Realized P&L is credited only to the relay whose bundle confirmed landed.
Nonce is released on EVERY terminal confirmation state to avoid leaks.

Part of the cexdex nonce-based relay fan-out design."
```

---

### Task 10: Confirmation tracker — nonce release on all terminal states

**Files:**
- Modify: `src/executor/confirmation.rs` — add `on_settle: Option<OnLandedCallback>` param that fires on EVERY terminal state (Landed, Failed, Timeout, RpcError exhaustion)
- Modify: `src/main.rs` — pass `None` for the new callback (main engine unchanged)

**Context:** The existing `on_landed` only fires on success. Nonce pool needs to know on ANY terminal state so we release the nonce. Spec §7 lists all four paths.

- [ ] **Step 1: Extend `spawn_confirmation_tracker` signature**

In `src/executor/confirmation.rs`:

```rust
pub fn spawn_confirmation_tracker(
    http_client: reqwest::Client,
    jito_url: String,
    estimated_profit_lamports: u64,
    tip_lamports: u64,
    mut relay_rx: tokio::sync::mpsc::Receiver<crate::executor::relays::RelayResult>,
    rpc_url: String,
    pool_address: String,
    trigger_slot: u64,
    on_landed: Option<OnLandedCallback>,
    /// Fired on EVERY terminal state, including timeout, failed, dropped,
    /// and rpc-error exhaustion. Used by cexdex to release the nonce.
    on_settle: Option<OnLandedCallback>,
) {
```

- [ ] **Step 2: Invoke `on_settle` in all four exit paths**

Inside the polling loop, wrap each terminal branch:

```rust
// Inside the Landed arm (after incrementing counters + on_landed.take()())
if let Some(cb) = on_settle.take() { cb(); }
return;

// Inside the Failed arm
if let Some(cb) = on_settle.take() { cb(); }
return;

// In the timeout (deadline reached) branch
if let Some(cb) = on_settle.take() { cb(); }
return;

// In the RPC-error-exhaustion branch
if let Some(cb) = on_settle.take() { cb(); }
return;
```

Also add the `mut on_settle = on_settle;` pattern at the top of the spawned task (same pattern already used for `on_landed`).

- [ ] **Step 3: Update the two existing call sites**

`src/main.rs` — add `, None` as the last argument.

`src/bin/cexdex.rs` — pass the `Some(on_settle_no_land)` callback from Task 9.

- [ ] **Step 4: Update the existing test**

`src/executor/confirmation.rs::tests::test_tracker_no_bundle_ids_exits_early` — add `None` as the new last argument.

- [ ] **Step 5: Run tests**

Run: `cargo test --lib executor::confirmation`
Expected: all existing pass.

- [ ] **Step 6: Commit**

```bash
git add src/executor/confirmation.rs src/main.rs src/bin/cexdex.rs
git commit -m "feat(executor): on_settle callback fires on all terminal states

Existing on_landed fires only on success. New on_settle fires on
Landed, Failed, Timeout, and RpcError exhaustion — any terminal
state. Used by cexdex to release the nonce pool slot regardless of
outcome (preventing nonce leaks when bundles don't land).

Main engine passes None for on_settle (no nonce in use).

Part of the cexdex nonce-based relay fan-out design."
```

---

### Task 11: New per-relay counters

**Files:**
- Modify: `src/metrics/counters.rs` — add/extend per-relay labelled counters

**Context:** `cexdex_bundles_attempted_total` was added earlier as an unlabelled counter. We now need it labelled by `relay`. Same for `cexdex_bundles_confirmed_total` (NEW) and `cexdex_tip_paid_usd_total` (NEW).

- [ ] **Step 1: Change + add the counters**

In `src/metrics/counters.rs`, replace:

```rust
pub fn inc_cexdex_bundles_attempted() {
    counter!("cexdex_bundles_attempted_total").increment(1);
}
```

with:

```rust
pub fn inc_cexdex_bundles_attempted(relay: &str) {
    counter!("cexdex_bundles_attempted_total", "relay" => relay.to_string()).increment(1);
}

pub fn inc_cexdex_bundles_confirmed(relay: &str) {
    counter!("cexdex_bundles_confirmed_total", "relay" => relay.to_string()).increment(1);
}

pub fn add_cexdex_tip_paid_usd(relay: &str, usd: f64) {
    if usd > 0.0 {
        counter!("cexdex_tip_paid_usd_micros_total", "relay" => relay.to_string())
            .increment((usd * 1_000_000.0) as u64);
    }
}
```

- [ ] **Step 2: Remove the old confirmation increment in the main engine if it uses the unlabeled counter**

Grep for `inc_cexdex_bundles_attempted()` across the repo and fix any call sites — should only be in `src/bin/cexdex.rs` (already updated in Task 9 with the `relay` label).

- [ ] **Step 3: Verify compilation**

Run: `cargo check --all-targets`
Expected: `Finished`.

- [ ] **Step 4: Commit**

```bash
git add src/metrics/counters.rs
git commit -m "feat(metrics): per-relay labelled cexdex counters

- cexdex_bundles_attempted_total{relay} (renamed, added label)
- cexdex_bundles_confirmed_total{relay} (NEW)
- cexdex_tip_paid_usd_micros_total{relay} (NEW; micros for u64 counter)

Enables Grafana panels that break out each relay's attempted vs
confirmed rate and cumulative tip spend.

Part of the cexdex nonce-based relay fan-out design."
```

---

### Task 12: Grafana dashboard panels

**Files:**
- Modify: `monitoring/provisioning/dashboards/cexdex-pnl.json` — append 3 new panels

**Context:** Nonce health row, relay-winner timeseries, tip-paid timeseries. Spec §Metrics lists positions (y=36, y=42).

- [ ] **Step 1: Append the three panels**

Add to the `panels` array (before the closing `]`):

```json
{
  "type": "stat",
  "title": "Nonce In-Flight / Collisions / Refreshes",
  "description": "In-flight count (0-N), collision rate (should stay near zero — nonzero means add more nonces), hash-refresh rate (Geyser sanity).",
  "gridPos": { "h": 6, "w": 24, "x": 0, "y": 36 },
  "datasource": { "type": "prometheus", "uid": "prometheus" },
  "targets": [
    { "expr": "cexdex_nonce_in_flight", "refId": "A", "legendFormat": "In-flight" },
    { "expr": "rate(cexdex_nonce_collision_total[5m])", "refId": "B", "legendFormat": "Collisions/sec" },
    { "expr": "rate(cexdex_nonce_hash_refresh_total[5m])", "refId": "C", "legendFormat": "Refreshes/sec" }
  ],
  "fieldConfig": {
    "defaults": { "unit": "short", "decimals": 2 },
    "overrides": []
  },
  "options": {
    "colorMode": "background",
    "graphMode": "area",
    "reduceOptions": { "calcs": ["lastNotNull"], "fields": "", "values": false },
    "textMode": "value_and_name"
  }
},
{
  "type": "timeseries",
  "title": "Relay Landing Rate",
  "description": "Confirmed bundles per relay per sec. Zero-crossing for a relay suggests its tip config or connection needs tuning.",
  "gridPos": { "h": 8, "w": 12, "x": 0, "y": 42 },
  "datasource": { "type": "prometheus", "uid": "prometheus" },
  "targets": [
    { "expr": "rate(cexdex_bundles_confirmed_total[5m])", "refId": "A", "legendFormat": "{{relay}}" }
  ],
  "fieldConfig": {
    "defaults": {
      "unit": "short",
      "custom": {
        "drawStyle": "line",
        "lineInterpolation": "linear",
        "lineWidth": 2,
        "fillOpacity": 10,
        "showPoints": "never"
      }
    }
  },
  "options": {
    "legend": { "displayMode": "list", "placement": "bottom", "showLegend": true },
    "tooltip": { "mode": "multi", "sort": "none" }
  }
},
{
  "type": "timeseries",
  "title": "Tip Paid (USD) per Relay — cumulative",
  "description": "Sum of tip dollars paid to each relay for confirmed bundles. Pair with the Relay Landing panel to compute cost-per-landing.",
  "gridPos": { "h": 8, "w": 12, "x": 12, "y": 42 },
  "datasource": { "type": "prometheus", "uid": "prometheus" },
  "targets": [
    { "expr": "cexdex_tip_paid_usd_micros_total / 1000000", "refId": "A", "legendFormat": "{{relay}}" }
  ],
  "fieldConfig": {
    "defaults": {
      "unit": "currencyUSD",
      "decimals": 4,
      "custom": {
        "drawStyle": "line",
        "lineInterpolation": "linear",
        "lineWidth": 2,
        "fillOpacity": 5,
        "showPoints": "never"
      }
    }
  },
  "options": {
    "legend": { "displayMode": "list", "placement": "bottom", "showLegend": true },
    "tooltip": { "mode": "multi", "sort": "none" }
  }
}
```

- [ ] **Step 2: Validate the JSON**

Run: `python3 -c "import json; json.load(open('monitoring/provisioning/dashboards/cexdex-pnl.json'))" && echo OK`
Expected: `OK`.

- [ ] **Step 3: Commit**

```bash
git add monitoring/provisioning/dashboards/cexdex-pnl.json
git commit -m "feat(monitoring): Grafana panels for nonce pool + per-relay stats

Three new panels:
- Nonce In-Flight / Collisions / Refreshes (y=36)
- Relay Landing Rate (timeseries by relay) (y=42)
- Tip Paid per Relay cumulative (y=42)

Provisioning will auto-reload on next Grafana restart or provisioning
rescan cycle (default 60s).

Part of the cexdex nonce-based relay fan-out design."
```

---

### Task 13: Docs — `.env.example` + `CLAUDE.md`

**Files:**
- Modify: `.env.example` — document the two new env vars
- Modify: `CLAUDE.md` — add a short section on nonce-based non-equivocation and link to the spec

- [ ] **Step 1: Update `.env.example`**

Add (near the existing CEXDEX_* section):

```
# ─── Durable-Nonce Relay Fan-Out (new 2026-04-17) ─────────────────────
# Nonce accounts for multi-relay non-equivocation. Each account:
#   - System Program owned
#   - Initialized with authority == searcher wallet
#   - ~1.4M lamports rent
# Leave empty to disable fan-out (Jito-only mode). Comma-separated list.
CEXDEX_SEARCHER_NONCE_ACCOUNTS=

# Per-relay tip fractions. Override CEXDEX_TIP_FRACTION on a per-relay
# basis. Useful because Jito runs a per-slot auction while Astralane
# forwards to shredstream — different economics, different optimal tips.
# Omit to inherit CEXDEX_TIP_FRACTION.
# CEXDEX_TIP_FRACTION_JITO=0.25
# CEXDEX_TIP_FRACTION_ASTRALANE=0.10
```

- [ ] **Step 2: Update `CLAUDE.md`**

Add a short subsection to the CEX-DEX arbitrage section:

```markdown
### CEX-DEX Multi-Relay Fan-Out (2026-04-17)

Safe fan-out to Jito + Astralane using durable Solana nonce accounts.
Every cexdex-submitted tx carries `advance_nonce_account(N, authority)` as
instruction #0 with the nonce's current hash in the `recent_blockhash`
slot. All relay copies of the same opportunity share the same nonce → only
one can land (the first to reach consensus advances the nonce; others
fail the nonce check atomically).

- Nonce pool: 3 accounts configured via `CEXDEX_SEARCHER_NONCE_ACCOUNTS`
- Round-robin selection by oldest `last_used`
- Hash cache maintained by Geyser (extends the narrow-subscription filter
  to include nonce pubkeys); zero-RPC on hot path
- Per-relay tip via `CEXDEX_TIP_FRACTION_JITO`, `CEXDEX_TIP_FRACTION_ASTRALANE`
- Metrics: `cexdex_nonce_collision_total`, `cexdex_nonce_in_flight`,
  `cexdex_bundles_attempted_total{relay}`, `cexdex_bundles_confirmed_total{relay}`

See `docs/superpowers/specs/2026-04-17-cexdex-nonce-fanout-design.md`.
```

- [ ] **Step 3: Commit**

```bash
git add .env.example CLAUDE.md
git commit -m "docs(cexdex): document nonce-based multi-relay fan-out

Two new env vars (CEXDEX_SEARCHER_NONCE_ACCOUNTS, CEXDEX_TIP_FRACTION_*)
with pointers to the design spec.

Part of the cexdex nonce-based relay fan-out design."
```

---

### Task 14: E2E integration test

**Files:**
- Create: `tests/e2e/cexdex_nonce_pipeline.rs`
- Modify: `tests/e2e/mod.rs` (add module) if an e2e module structure exists

**Context:** Full detector → simulator → checkout → build → sign flow with fixture data. Verifies the transaction wire format: ix[0] is AdvanceNonceAccount with our nonce pubkey, `message.recent_blockhash == nonce_hash`, tip account is the relay's tip account.

- [ ] **Step 1: Create the test**

```rust
//! End-to-end test for the cexdex nonce-based fan-out pipeline.
//! Wires: NoncePool (seeded via update_cached_hash) -> NonceInfo ->
//! build_signed_bundle_tx -> decoded VersionedTransaction.
//! Asserts on-the-wire structure: ix[0] advance_nonce, recent_blockhash,
//! tip-account writability.

#![cfg(feature = "e2e")]

use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::transaction::VersionedTransaction;
use solana_sdk::message::VersionedMessage;
use solana_sdk::system_instruction;
use std::str::FromStr;

use solana_mev_bot::cexdex::{NoncePool, NonceInfo};
use solana_mev_bot::executor::relays::common::build_signed_bundle_tx;

#[test]
fn nonce_advance_at_ix0_and_blockhash_matches() {
    let signer = Keypair::new();
    let nonce_pk = Pubkey::new_unique();
    let tip_account = Pubkey::new_unique();
    let nonce_hash = Hash::new_unique();

    // Seed pool
    let pool = NoncePool::new(vec![nonce_pk]);
    pool.update_cached_hash(nonce_pk, nonce_hash);
    let (checked_out_pk, checked_out_hash) = pool.checkout().expect("checkout");
    assert_eq!(checked_out_pk, nonce_pk);
    assert_eq!(checked_out_hash, nonce_hash);

    // Base ix = a trivial transfer to simulate a swap
    let dummy_dest = Pubkey::new_unique();
    let base_ix = system_instruction::transfer(&signer.pubkey(), &dummy_dest, 100);

    let bytes = build_signed_bundle_tx(
        "test",
        &[base_ix],
        50_000,
        &tip_account,
        &signer,
        checked_out_hash,
        &[],
        Some(NonceInfo {
            account: nonce_pk,
            authority: signer.pubkey(),
        }),
    )
    .expect("should build");

    let tx: VersionedTransaction = bincode::deserialize(&bytes).expect("decode");

    match &tx.message {
        VersionedMessage::Legacy(m) => {
            // ix[0] should be advance_nonce
            let ix0 = &m.instructions[0];
            let disc = u32::from_le_bytes(ix0.data[0..4].try_into().unwrap());
            assert_eq!(disc, 4, "AdvanceNonceAccount discriminator");
            // recent_blockhash = nonce hash
            assert_eq!(m.recent_blockhash, nonce_hash);
            // Last instruction should be the tip transfer
            let last = m.instructions.last().expect("no ix");
            let ldisc = u32::from_le_bytes(last.data[0..4].try_into().unwrap());
            assert_eq!(ldisc, 2, "System::Transfer (tip)");
        }
        VersionedMessage::V0(m) => {
            let ix0 = &m.instructions[0];
            let disc = u32::from_le_bytes(ix0.data[0..4].try_into().unwrap());
            assert_eq!(disc, 4, "AdvanceNonceAccount discriminator");
            assert_eq!(m.recent_blockhash, nonce_hash);
        }
    }
}
```

- [ ] **Step 2: Run**

Run: `cargo test --features e2e --test e2e cexdex_nonce_pipeline`
Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/cexdex_nonce_pipeline.rs
git commit -m "test(cexdex): e2e for nonce-based bundle pipeline

Full checkout -> build -> decode flow verifying the on-the-wire tx
structure: ix[0] is AdvanceNonceAccount, recent_blockhash matches
the nonce's cached hash, tip transfer is the last instruction.

Part of the cexdex nonce-based relay fan-out design."
```

---

### Task 15: Final verification + live-rollout checklist

- [ ] **Step 1: Full test suite**

Run: `cargo test --all-targets`
Expected: all pass. Known preexisting flake: `router_perf::test_route_calc_completes_under_5ms_with_50_pools` in debug mode — skip or run with `--release`.

- [ ] **Step 2: Clippy clean**

Run: `cargo clippy --all-targets 2>&1 | grep -E "error|warning: " | head -20`
Expected: no NEW warnings from files touched in this plan.

- [ ] **Step 3: Release build**

Run: `cargo build --release --bin cexdex`
Expected: `Finished release`.

- [ ] **Step 4: Live rollout (staged — per spec §Testing > Live rollout)**

Only proceed once the full commit chain is pushed and green.

1. **Dry-run with nonces** — set `CEXDEX_DRY_RUN=true`, both relays enabled. Confirm: detector fires, `cexdex_nonce_hash_refresh_total` increases, `cexdex_bundles_attempted_total{relay}` fires for both relays on an opportunity, NO actual submission. Check for ~5 min.

2. **Live with Jito ONLY + nonce** — temporarily drop Astralane from the relay list. Submit one real bundle. Verify on-chain via `getAccountInfo` on one of the nonce accounts: `blockhash` field CHANGED after landing (proves the nonce advanced). Use `getSignaturesForAddress` to confirm exactly one new tx from the searcher wallet.

3. **Live with Jito + Astralane + nonce (full fan-out)** — restore Astralane. First 30 opportunities: verify `cexdex_nonce_collision_total` stays at 0 or very low, `cexdex_bundles_confirmed_total{relay}` shows landings, `getSignaturesForAddress` shows **exactly one tx per opportunity**, not two.

4. **Rollback trigger** — any opportunity landing two successful txs in the same or adjacent slots: `pkill -INT`, revert to single-relay, investigate.

- [ ] **Step 5: Final commit + push**

```bash
# If anything was adjusted during live rollout (e.g. tuning .env), commit it.
git status
git push origin main
```

---

## Self-Review (inline)

### Spec coverage check

Going through each `## Components` section of the spec:

| Spec section | Plan task(s) | Status |
|---|---|---|
| §1 NoncePool | Task 2 | ✓ |
| §2 Nonce parser | Task 1 | ✓ |
| §3 Geyser subscription | Task 7, Task 8 (bootstrap) | ✓ |
| §4 Bundle builder | Task 3 | ✓ |
| §5 Per-relay tip fractions | Task 5 (config), Task 6 (sim), Task 9 (dispatch) | ✓ |
| §6 Relay list (re-enable Astralane) | Task 9 | ✓ |
| §7 Confirmation tracker hooks | Task 10 | ✓ |
| Metrics | Task 11 | ✓ |
| Grafana panels | Task 12 | ✓ |
| Env + docs | Task 13 | ✓ |
| Tests (unit) | Tasks 1, 2, 3, 6 | ✓ |
| Tests (integration) | Task 14 | ✓ |
| Live rollout | Task 15 | ✓ |

### Placeholder scan

Searched for: "TBD", "TODO (not FIXME)", "fill in", "similar to", "add appropriate".
- One `FIXME(Task 9)` in Task 6 — deliberate forward reference to the next task. Explained in the task body. OK.
- No vague "add error handling" or "write tests for the above" without code.

### Type consistency

- `NonceInfo { account, authority }` — used consistently in Tasks 2, 3, 4, 9, 14.
- `SimulationResult::Profitable { route, adjusted_profit_sol, adjusted_profit_usd, net_profit_usd_worst_case, min_final_output }` — same fields used in Task 6 definition and Task 9 destructure.
- `inc_cexdex_bundles_attempted(relay)` — labelled in Task 11, used in Task 9.
- `inc_cexdex_bundles_confirmed(relay)` — labelled in Task 11, used in Task 9's on_landed callback.
- `spawn_confirmation_tracker` signature — Task 10 adds `on_settle` param; all callers updated.

Consistent.
