# CEX-DEX Nonce-Based Relay Fan-Out Design

**Date:** 2026-04-17
**Status:** Approved; ready for implementation plan
**Supersedes:** single-relay constraint added after LIVE3 incident (commit `f329f9f`)

## Motivation

The cexdex binary currently submits to Jito only. Fanning out to multiple relays (Jito + Astralane) was disabled after the LIVE3 incident (2026-04-17, slot 413825986) where both relays landed **different** signed txs of the same opportunity — each relay requires its own tip account, producing byte-different transactions with independent tx signatures. Solana's native signature-uniqueness invariant did not catch this because the signatures *were* unique. Result: one swap executed twice, at a ~$0.25 loss in that instance.

Durable nonce accounts provide the missing primitive: by prepending `advance_nonce_account(N, authority)` as instruction #0 and using the nonce's current hash as `recent_blockhash`, two txs for the same opportunity mutually exclude — the first to land advances the nonce, the second(s) fail atomically at the nonce check.

This restores multi-relay fan-out, which is valuable because:
1. **Rate limits:** Jito caps unauthenticated submissions at 1/sec. Astralane allows higher throughput. Hitting multiple relays dodges per-relay caps.
2. **Inclusion diversity:** Jito runs a per-slot auction; Astralane forwards to shredstream. Different failure modes mean higher combined landing rate.
3. **Per-relay tip economics:** each relay's profit-maximizing tip is different. The design supports independent `CEXDEX_TIP_FRACTION_*` values per relay.

## Goals

- Safe fan-out to Jito + Astralane: same opportunity → ≤1 landing.
- Independent per-relay tip configuration (`CEXDEX_TIP_FRACTION_JITO`, `CEXDEX_TIP_FRACTION_ASTRALANE`).
- Geyser-maintained nonce hash cache (zero RPC on hot path).
- Round-robin nonce selection with collision visibility.
- Main-engine (DEX↔DEX) path unchanged — nonce usage is cexdex-only.

## Non-Goals (Out of Scope)

- Increasing dispatch concurrency beyond the current 1500ms global cooldown. The nonce pool's in-flight capacity (3) will naturally cap concurrency, but we are not relaxing the cooldown in this phase.
- Same-relay multi-shot tip hedging (sending N copies with varied tips to Jito). Jito's per-slot auction picks the highest tip among its candidates — same-relay hedging collapses to "just send the highest." Explicitly rejected.
- Falling back to ephemeral blockhash when the nonce pool is exhausted. That reintroduces exactly the double-fill bug we are fixing.
- Auto-creation of new nonce accounts. We use the 3 pre-created accounts listed in `CEXDEX_SEARCHER_NONCE_ACCOUNTS`. If collision metric indicates we need more, user creates more accounts and extends the env var.

## Architecture

```
opportunity
     │
     ▼
NoncePool.checkout()  ──►  (nonce_pk, nonce_hash)
     │                      (from Geyser-maintained cache)
     │
     ├─► build tx for Jito:       [nonce_advance, swap, jito_tip]       hash=nonce_hash
     ├─► build tx for Astralane:  [nonce_advance, swap, astralane_tip]  hash=nonce_hash
     │
     ▼
dispatch both concurrently
     │
     ▼
Solana runtime processes exactly one
     │   (the first to reach a validator;
     │    the other(s) fail at nonce check)
     ▼
confirmation tracker → on_landed(relay)
     │                   on_dropped()
     ▼
NoncePool.mark_settled(nonce_pk)
```

**Invariant:** for any opportunity dispatched to ≥1 relay, at most one bundle lands. This holds because all bundles for a given opportunity share the same `advance_nonce_account(N)` at ix[0] against the same current hash `H`. The runtime advances `N` to `H'` upon the first successful landing; subsequent txs referencing `H` fail the nonce check.

## Components

### 1. NoncePool (`src/cexdex/nonce.rs`, new, ~150 lines)

```rust
pub struct NoncePool {
    nonces: Vec<Pubkey>,                   // fixed-order from config
    state: Arc<DashMap<Pubkey, NonceState>>,
}

struct NonceState {
    last_used: Instant,        // for round-robin selection
    in_flight: bool,           // true between checkout and mark_settled
    cached_hash: Hash,         // last known on-chain blockhash
    hash_observed_at: Instant, // freshness of cached_hash
}

impl NoncePool {
    pub fn new(nonces: Vec<Pubkey>) -> Self;

    /// Round-robin by last_used. If the oldest is still in_flight,
    /// return it anyway AND increment cexdex_nonce_collision_total.
    /// Returns None if cached_hash is still Hash::default() for all
    /// nonces (pre-Geyser warmup — detector should skip).
    pub fn checkout(&self) -> Option<(Pubkey, Hash)>;

    /// Called from the confirmation tracker's on_landed / on_dropped.
    pub fn mark_settled(&self, pubkey: Pubkey);

    /// Called from the Geyser nonce-parser callback.
    pub fn update_cached_hash(&self, pubkey: Pubkey, hash: Hash);
}
```

**Selection rule:** `min_by(state.last_used)`. Collision (in_flight at selection time) still returns the pubkey — the caller's tx will fail at nonce check if the prior bundle already landed, or queue behind it if it hasn't. Collision is logged via a counter; it is not an error.

**Concurrency:** `DashMap` write-lock per entry; detector loop is single-threaded so contention is effectively zero.

### 2. Nonce account parser (`src/mempool/parsers/nonce.rs`, new, ~60 lines)

Solana nonce account layout (80 bytes, System Program-owned):

| Offset | Length | Field |
|--------|--------|-------|
| 0 | 4 | version (u32, 0) |
| 4 | 4 | state (u32, 1=Initialized) |
| 8 | 32 | authority (Pubkey) |
| 40 | 32 | nonce (Hash) |
| 72 | 8 | fee_calculator.lamports_per_signature (u64) |

```rust
pub fn parse_nonce(data: &[u8]) -> Option<(Pubkey, Hash)> {
    if data.len() < 72 { return None; }
    let state = u32::from_le_bytes(data[4..8].try_into().ok()?);
    if state != 1 { return None; }  // not initialized
    let authority = Pubkey::new_from_array(data[8..40].try_into().ok()?);
    let hash = Hash::new_from_array(data[40..72].try_into().ok()?);
    Some((authority, hash))
}
```

Return value: `(authority, hash)`. The caller verifies authority == searcher wallet before trusting the update (defense in depth; Geyser only delivers what we subscribed to, but we still sanity-check).

### 3. Geyser subscription (`src/cexdex/geyser.rs`, `src/mempool/stream.rs`)

The existing `SubscriptionMode::SpecificAccounts` already takes a `Vec<Pubkey>`. Extend the list at subscription build time to include the 3 nonce accounts alongside the 4 pool accounts (7 total).

In `src/mempool/stream.rs`, at the top of the parser dispatch for each incoming account update:

```rust
// Nonce short-circuit: 80 bytes + System Program owner + known pubkey
if account.data.len() == 80
    && account.owner == SYSTEM_PROGRAM
    && nonce_pool.contains(&account.pubkey)
{
    if let Some((authority, hash)) = parse_nonce(&account.data) {
        if authority == searcher_pubkey {  // sanity check
            nonce_pool.update_cached_hash(account.pubkey, hash);
            inc_cexdex_nonce_hash_refresh_total();
        }
    }
    return;  // don't fall through to pool parsers
}
```

**Startup bootstrap:** before the detector loop starts, fetch each nonce's current state once via `getAccountInfo` to pre-populate the cache. This removes a ~1-2 slot window where `checkout()` would return `None`.

### 4. Bundle builder with nonce (`src/executor/relays/common.rs`)

Extend `build_signed_bundle_tx` with an optional `Option<NonceInfo>` parameter:

```rust
pub struct NonceInfo {
    pub account: Pubkey,
    pub authority: Pubkey,  // == signer.pubkey() for cexdex
}
```

**When `nonce` is `Some(info)`:**
1. Prepend `solana_system_interface::instruction::advance_nonce_account(&info.account, &info.authority)` as **instruction index 0**.
2. Pass `recent_blockhash = nonce_hash` (provided by caller; looks up via `NoncePool.cached_hash`).
3. Sign with the searcher keypair as usual — the same signature satisfies both fee-payer and nonce-authority roles because `authority == signer.pubkey()`.

**When `nonce` is `None`:** unchanged — main engine keeps this path.

**Size impact:** `advance_nonce_account` adds 1 readonly account ref + 1 writable account ref + 1 signer ref + 2 bytes ix data. Negligible vs the 1232-byte legacy tx limit.

### 5. Per-relay tip fractions

**Env vars** (backward-compat: fall back to `CEXDEX_TIP_FRACTION`):

```
CEXDEX_TIP_FRACTION_JITO=0.25
CEXDEX_TIP_FRACTION_ASTRALANE=0.10
# existing CEXDEX_TIP_FRACTION=0.15 acts as the default fallback
```

**Config:** `CexDexConfig.tip_fractions: HashMap<String, f64>`, populated at startup with one entry per configured relay name, defaulting to `default_tip_fraction` if the per-relay var is absent.

**Simulator change:** currently returns `SimulationResult::Profitable { tip_lamports, ... }`. New: returns `adjusted_profit_sol` and `min_final_output`; tip is computed per-relay at dispatch time.

**Worst-case gate in simulator:**
```rust
let max_fraction = tip_fractions.values().max();
let worst_case_net_usd = adjusted_profit_usd * (1.0 - max_fraction) - tx_fee_usd;
if worst_case_net_usd < min_profit_usd { reject; }
```

Any dispatch that passes this gate is profitable regardless of which relay lands first. If a lower-tip relay wins the race, net exceeds the sim estimate — a pleasant surprise, never the reverse.

**Dispatch-time per-relay tip:**
```rust
for relay in configured_relays {
    let f = tip_fractions.get(relay.name()).copied().unwrap_or(default_tip_fraction);
    let tip_sol = adjusted_profit_sol * f;
    let tip_lamports = max(sol_to_lamports(tip_sol), min_tip_lamports);
    let tip_account = relay.tip_account();
    build_signed_bundle_tx(
        relay.name(), instructions, tip_lamports, &tip_account,
        signer, nonce_hash, alts, Some(nonce_info),
    )
    relay.submit(...)
}
```

### 6. Relay list

`src/bin/cexdex.rs` — restore Astralane:

```rust
let relays: Vec<Arc<dyn Relay>> = vec![
    Arc::new(JitoRelay::new(&bot_config_relays)),
    Arc::new(AstralaneRelay::new(&bot_config_relays, shutdown_rx.clone())),
];
```

Remove the `// Single-relay only ... until nonce-based fix` comment; replace with a short pointer to this design doc.

### 7. Confirmation tracker hooks

Existing `spawn_confirmation_tracker` already accepts an `OnLandedCallback`. Extend to:
- Accept a `NoncePool` handle and a `nonce_pubkey`
- On `Landed` OR `Failed` OR `Dropped`: call `pool.mark_settled(nonce_pubkey)`
- On `Landed`: additionally call the existing `on_landed` closure (realized PNL crediting) AND `inc_cexdex_bundles_confirmed(relay_name)`

To emit the `relay_name` label, the tracker needs to know which relay's bundle landed. The existing `relay_rx` channel already carries `RelayResult { relay_name, bundle_id, ... }` — we can correlate which bundle's ID `getBundleStatuses` reports as landed, and label the metric accordingly.

## Metrics

**New counters/gauges in `src/metrics/counters.rs`:**
- `cexdex_nonce_collision_total` (counter) — checkout on an in-flight nonce
- `cexdex_nonce_in_flight` (gauge, 0 to N) — current in-flight count
- `cexdex_nonce_hash_refresh_total` (counter) — Geyser-driven cache updates
- `cexdex_bundles_attempted_total{relay}` (labelled) — replaces unlabelled counter
- `cexdex_bundles_confirmed_total{relay}` (labelled) — only for the relay that landed
- `cexdex_tip_paid_usd_total{relay}` (labelled) — cumulative tip USD per relay

**Grafana dashboard additions** (appended to `monitoring/provisioning/dashboards/cexdex-pnl.json`):
1. **Nonce health row** (h=6 w=24 at y=36): in-flight gauge + collision rate + refresh rate
2. **Relay winner** (timeseries, h=8 w=12 at y=42): `rate(cexdex_bundles_confirmed_total{relay=*}[5m])`
3. **Tip paid breakdown** (timeseries, h=8 w=12 at y=42): `cexdex_tip_paid_usd_total{relay=*}`

**Thresholds worth watching:**
- `cexdex_nonce_in_flight == len(nonces)` sustained → add more accounts
- `rate(cexdex_nonce_collision_total[5m]) > 0.1/sec` → add more accounts
- Zero-crossing in `cexdex_bundles_confirmed_total{relay="astralane"}` after 30 min live → retune Astralane tip fraction or drop it

## Data Flow — full picture

```
┌─────────────┐           ┌────────────┐
│  Binance WS │  →  price │ PriceStore │
└─────────────┘           └─────┬──────┘
                                │
┌────────────────┐              │
│ Geyser stream  │  pool state  │
│ (7 accounts:   ├──────────────┤
│  4 pools +     │              ▼
│  3 nonces)     │         ┌────────────┐
└──────┬─────────┘         │  Detector  │
       │                   └─────┬──────┘
       │ nonce hash              │ CexDexRoute
       ▼                         ▼
┌──────────────┐           ┌──────────────┐
│  NoncePool   │◄─checkout─│  Simulator   │
└──────┬───────┘           └─────┬────────┘
       │                         │ Profitable
       │ (pk, hash)              ▼
       │                   ┌──────────────┐
       └──────────────────►│  Bundle bldr │
                           │  (per relay) │
                           └──┬──────┬────┘
                              │      │
                       Jito tx       Astralane tx
                       (same nonce advance)
                              │      │
                              ▼      ▼
                           ┌────────────┐
                           │  Solana    │
                           │  runtime   │
                           └──┬─────────┘
                              │ exactly one lands
                              ▼
                    ┌──────────────────┐
                    │ Confirmation     │
                    │ tracker          │
                    └──┬───────────────┘
                       │
                       ├── mark_settled(nonce_pk)
                       ├── add_realized_pnl_usd(net) [landed only]
                       └── inc_bundles_confirmed{relay}
```

## Error Handling

| Scenario | Behavior |
|---|---|
| Nonce pool empty at detector call | `checkout()` returns `None` → detector skips, logs `cexdex_detector_skip_total{reason="nonce_pool_empty"}` |
| Geyser stream silent for a nonce account | cached_hash stale; next `checkout` returns the stale hash. Tx still valid — the on-chain nonce might have the same or advanced value. If same: lands. If advanced: fails nonce check (cheap failure, next opportunity will have fresh hash post-Geyser redelivery). |
| Both relays reject (rate limited, connection error) | No bundle submitted; nonce state left in_flight. `mark_settled` fires from the confirmation tracker's timeout path (~12s later). Acceptable latency cost; no corruption. |
| Bundle lands but confirmation tracker misses it (RPC rate limit exhausts retries) | `mark_settled` fires from timeout (12s). During that window, nonce is held. Realized PNL is NOT credited (known limitation of confirmation tracker — unchanged). |
| Nonce account de-initialized externally (authority rug) | Parser returns `None`; cache not updated. Next checkout returns stale hash; tx fails. No tx executes. User would see a dead nonce via Grafana (hash_refresh flatlines) and investigate. Out-of-band concern. |
| Main engine also runs on same host | Main engine uses `None` for nonce param (confirmed in `main.rs`). No interaction with cexdex's nonce pool. |

## Testing

### Unit tests (`tests/unit/`)
1. `nonce_pool_round_robin_picks_oldest`
2. `nonce_pool_collision_counter_fires_when_all_in_flight`
3. `nonce_pool_mark_settled_resets_in_flight`
4. `nonce_pool_returns_none_before_cache_populated`
5. `nonce_parser_rejects_uninitialized`
6. `nonce_parser_extracts_authority_and_hash` (fixture: bytes from a real on-chain nonce)
7. `bundle_builder_prepends_nonce_advance_at_ix0`
8. `bundle_builder_uses_nonce_hash_as_blockhash`
9. `bundle_builder_without_nonce_unchanged` (regression guard)

### Integration test (feature-gated `e2e`)
`tests/e2e/cexdex_nonce_pipeline.rs` — full detector→checkout→build→sign flow using a known nonce fixture. Decodes the compiled tx and asserts: ix[0] is AdvanceNonceAccount, ix[0].accounts[0] == nonce_pk, `message.recent_blockhash == nonce_hash`, tip account matches the relay.

### Live rollout (staged)
1. **Dry-run with nonces** (`CEXDEX_DRY_RUN=true`, both relays enabled). Confirm: checkout works, bundles built with correct structure (log full tx at debug level), no actual submission.
2. **Live with Jito only + nonce.** Submit one real bundle. Verify on-chain via `getAccountInfo` on the nonce account: `blockhash` field CHANGED after landing → proves the nonce actually advanced, proving the mechanism works end-to-end.
3. **Live with Jito + Astralane + nonce.** Monitor `cexdex_nonce_collision_total` and `cexdex_bundles_confirmed_total{relay=*}`. Use `getSignaturesForAddress` to verify exactly-one landing per opportunity across the first 30 opportunities.
4. **Rollback trigger:** any opportunity producing two successful landings in the same or adjacent slots → `pkill -INT`, revert to single-relay, investigate.

### Success criteria for shipping
- `multiple_landings_per_opportunity == 0` across 30+ opportunities in phase 3
- Landing rate ≥ Jito-only baseline (≥ no regression)
- Per-confirmed-bundle tx-fee spend within 15% of Jito-only (accounts for the extra failed-tx cost)

## Configuration Summary

New env vars:
```
CEXDEX_SEARCHER_NONCE_ACCOUNTS=<pk1>,<pk2>,<pk3>
CEXDEX_TIP_FRACTION_JITO=0.25          # optional, falls back to CEXDEX_TIP_FRACTION
CEXDEX_TIP_FRACTION_ASTRALANE=0.10     # optional, falls back to CEXDEX_TIP_FRACTION
```

Existing vars unchanged: `CEXDEX_TIP_FRACTION`, `CEXDEX_POOLS`, dedup/cooldown, fraction cap, min profit, etc.

## File Inventory

| File | Change |
|---|---|
| `src/cexdex/nonce.rs` | NEW (~150 lines) — NoncePool, NonceState |
| `src/mempool/parsers/nonce.rs` | NEW (~60 lines) — 80-byte parser |
| `src/cexdex/geyser.rs` | Extend pool list with nonce pubkeys |
| `src/mempool/stream.rs` | Nonce short-circuit in parser dispatch |
| `src/cexdex/config.rs` | Parse `CEXDEX_SEARCHER_NONCE_ACCOUNTS`, `CEXDEX_TIP_FRACTION_*` |
| `src/cexdex/simulator.rs` | Return `adjusted_profit_sol` instead of `tip_lamports` |
| `src/executor/relays/common.rs` | `Option<NonceInfo>` param on `build_signed_bundle_tx` |
| `src/executor/confirmation.rs` | Accept `NoncePool` handle, call `mark_settled` on settle |
| `src/bin/cexdex.rs` | Per-relay dispatch w/ per-relay tip, re-enable Astralane, wire nonce pool |
| `src/metrics/counters.rs` | New labelled counters |
| `monitoring/provisioning/dashboards/cexdex-pnl.json` | 3 new panels (nonce health, relay winner, tip paid) |
| `.env.example` | Document new env vars |
| `CLAUDE.md` | Document the nonce-based non-equivocation strategy |
| `tests/unit/cexdex_nonce.rs` | 9 unit tests |
| `tests/e2e/cexdex_nonce_pipeline.rs` | 1 integration test |
