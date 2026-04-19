# EngineMev Performance Audit — 2026-04-19

Current baseline: p50=906us pipeline (Geyser event -> bundle submitted). All "DONE"
items from CLAUDE.md are excluded. Every finding below is implementable in
under one day.

## Executive Summary

Top three highest-impact wins:

1. **PoolState cloning on every cache read** (issue #1). Router + simulator
   clone a ~300-byte `PoolState` 6-12x per event. Replacing `get_any()`'s clone
   with `DashMap::Ref` (zero-copy) or a `Arc<PoolState>` cache value yields an
   estimated 100-250us.
2. **Metrics helpers allocate a `String` per label per call** (issue #2). Every
   `inc_geyser_updates`, `record_geyser_parse_duration_us`, etc. calls
   `.to_string()` on a `&'static str` label. At 2k+ Geyser updates/sec this is
   pure heap churn. Switching to `Cow<'static, str>` or a pre-built
   `Label` is a ~30us/event cleanup + less GC pressure.
3. **ATA derivation (`find_program_address`) is called 5-15x per bundle build**
   (issue #3). Each call runs up to 255 SHA-256 iterations (~5-10us). A per-mint
   ATA cache in the bundle builder eliminates 50-150us off every submission.

---

## Findings (ranked by estimated impact)

### 1. PoolState cloned on every cache read
**File:** `src/state/cache.rs:116-136`, `src/router/calculator.rs:74,91,132,153,174`, `src/router/simulator.rs:95-99,216`
**Cost:** `StateCache::get_any()` returns `Option<PoolState>` by cloning out of the
DashMap entry (line 135 `entry.state.clone()`). `PoolState` contains `PoolExtra`
(13+ `Option<Pubkey>`, each 33 bytes), bumping the clone to ~300+ bytes of
`memcpy`. In 2-hop route search we call `get_any` up to `MAX_POOLS_PER_TOKEN`
(20) + `return_pools.len()` times (~25-40 clones per event), plus simulator
clones the state 2-3 more times and clones the full route at line 216.
**Fix:** Store `Arc<PoolState>` in the DashMap value so `get_any` returns
`Option<Arc<PoolState>>` (one atomic increment, no memcpy). Change downstream
to `&PoolState` via `.as_ref()` — no API ripple in hot paths since most reads
immediately borrow.
**Estimated delta:** ~100-250us per event (dominant at current p50).

### 2. Metrics label allocations on hot path
**File:** `src/metrics/counters.rs:12,16,24,28,36,40` (and every counter helper)
**Cost:** Each helper does `"dex_type" => dex_type.to_string()` — one heap alloc
per call. `inc_geyser_updates` + `record_geyser_parse_duration_us` fire on
every parsed Geyser update (thousands/sec), and `inc_opportunities` /
`inc_bundles_skipped` fire per route. That is thousands of 8-16 byte `String`
allocs/sec.
**Fix:** Change helpers to accept `&'static str` and use `Cow::Borrowed`:
`counter!("...", "dex_type" => Cow::<'static, str>::Borrowed(dex_type))`. The
`metrics` crate accepts `impl Into<SharedString>`. All current callsites pass
string literals. Bonus: replace `format!("{:?}", route.hops[0].dex_type)` in
`main.rs:455,460` with a `const fn DexType::name(&self) -> &'static str`.
**Estimated delta:** ~20-40us/event at high update rate, plus reduced alloc
pressure.

### 3. ATA derivation not cached in BundleBuilder
**File:** `src/executor/swaps/mod.rs:35-43`, `src/executor/bundle.rs:141,158,172,238,257,290,469`
**Cost:** `derive_ata_with_program` calls `Pubkey::find_program_address` which
runs SHA-256 up to 255 times searching for a valid bump. ~5-10us per call. A
2-hop bundle computes: 1 ATA per unique mint in ATA-create loop (2), plus 2
inside each swap IX builder (4), plus wSOL wrap (1) + wSOL unwrap (1) + v2 IX
output_ata (2). That is ~10 derivations, ~50-100us per bundle build.
**Fix:** Cache `(wallet, mint, token_program) -> Pubkey` in a `DashMap` on
`BundleBuilder` (wallet is constant — a `HashMap<(Pubkey, Pubkey), Pubkey>`
with `mint+program` key suffices). Pre-populate for wSOL/USDC/USDT at startup.
**Estimated delta:** 50-100us/bundle.

### 4. Router `dispatch()` clones instruction vec + ALTs per relay
**File:** `src/executor/relay_dispatcher.rs:53-58,104-106`
**Cost:** For every opportunity, `dispatch` loops all configured relays and
per-relay does `base_instructions.to_vec()` and `self.alts.clone()`
(`Vec<Arc<...>>`). Instructions are ~2-4KB each, alts are cheap (Arc clones)
but the `Vec::clone` still allocates. With 5 relays this is 5x a 2-8KB alloc
per bundle.
**Fix:** Share instructions via `Arc<Vec<Instruction>>`. `RelayDispatcher`
already owns the alts as `Vec<Arc<AddressLookupTableAccount>>` — hand
tasks an `Arc<[Arc<...>]>` or just clone the outer `Vec` once into an `Arc`.
**Estimated delta:** ~30-80us/bundle, less alloc churn.

### 5. ALT cloned again inside `build_signed_bundle_tx`
**File:** `src/executor/relays/common.rs:237`
**Cost:** Each relay call does `alts.iter().map(|a| (*a).clone()).collect()`
to satisfy `v0::Message::try_compile`'s `&[AddressLookupTableAccount]` (owned)
signature. ALT has 170+ addresses (~5KB). Done once per relay submit, so 5x
per opportunity.
**Fix:** Verify whether a newer `solana-message` version accepts slices of
references; if not, build a single `Vec<AddressLookupTableAccount>` inside
`RelayDispatcher::new()` (ALTs are static post-boot) and hand it to submit
calls by reference.
**Estimated delta:** ~20-50us/bundle.

### 6. JSON-RPC body built from `serde_json::json!()` per submit
**File:** `src/executor/relays/jito.rs:95`, `nozomi.rs:85`, `bloxroute.rs:88`, `astralane.rs:181`, `zeroslot.rs`
**Cost:** Every submit builds a `serde_json::Value` tree, then `.json(&payload)`
re-serializes it to bytes. For Jito's `sendBundle` the payload shape is
invariant — only the inner tx string changes.
**Fix:** Use `String` concatenation or a pre-built template with byte-level
splicing: `format!(r#"{{"jsonrpc":"2.0","id":1,"method":"sendBundle","params":[["{}"]]}}"#, encoded)`
and call `.body(body_string).header("content-type", "application/json")`.
Skips the `Value` tree walk and re-serialize.
**Estimated delta:** ~30-60us/relay submit; 5 relays so 150-300us/bundle
(partially parallel).

### 7. Router thread polls channel with 100ms `recv_timeout`
**File:** `src/main.rs:310-316`
**Cost:** On idle slots the router wakes every 100ms to check shutdown.
Harmless but introduces up-to-100ms shutdown latency and extra wakeups.
Using `crossbeam_channel::select!` with a shutdown channel eliminates both
polling cost and shutdown lag.
**Fix:** Add a dedicated `crossbeam_channel::unbounded()` for shutdown and
`select!` on (change_rx, shutdown_rx).
**Estimated delta:** cleanup (marginal CPU win, cleaner semantics).

### 8. `pools_for_token` and `pools_for_pair` allocate fresh `Vec` per call
**File:** `src/state/cache.rs:139-154`
**Cost:** Route calculator calls `pools_for_token(base_mint)` per event; for
SOL this returns potentially hundreds of pools. The method collects the
`HashSet` into a new `Vec<Pubkey>` on every call (line 143). Same for
`pools_for_pair` (clones the stored `Vec`).
**Fix:** Return a callback-based iterator — `for_each_pool_of_token<F: FnMut(&Pubkey)>`
— so the calculator iterates without allocating. Requires the calculator to
apply `MAX_POOLS_PER_TOKEN` truncation inline (trivial).
**Estimated delta:** ~20-60us/event when SOL has 100+ pools indexed.

### 9. `recent_pools` / `recent_arbs` never shrink between evictions
**File:** `src/main.rs:292-300,327-330,498-506`
**Cost:** `recent_pools` eviction only triggers at 10k entries, and
`recent_arbs` retains within a 2s window by walking all entries on every
opportunity (line 499). Worst-case the router walks 10k entries per event.
**Fix:** Replace with `lru::LruCache` (fixed bound, O(1) ops) or a
small-capacity `IndexMap` with ring-buffer eviction. Move the retain call
behind a counter (only run every N opportunities).
**Estimated delta:** ~10-40us/event under high opportunity rate; bounded
memory.

### 10. Blockhash `RwLock` could be an `ArcSwap`
**File:** `src/state/blockhash.rs:16-52`
**Cost:** Every opportunity calls `blockhash_cache.get()` which takes a read
lock. Under heavy load (many relays + async tasks all reading), writer lock in
the 2s refresh briefly blocks readers. RwLock read acquisition is ~30-50ns
uncontended but up to microseconds under contention with pthread on Linux.
**Fix:** Swap `Arc<RwLock<Option<BlockhashInfo>>>` for `arc_swap::ArcSwap`.
Readers do an `Arc` load (single atomic), writers do a single atomic swap.
Same applies to `TipFloorCache` (`src/state/tip_floor.rs:44-46`).
**Estimated delta:** cleanup / contention insurance. Low impact at current
QPS but removes a latency spike risk.

### 11. `fetch_vault_balances_for_pool` spawns a bare `tokio::spawn` per event
**File:** `src/mempool/stream.rs:504-512` (also 459-462, 529-553, 581-599, 645-665)
**Cost:** Every Geyser update that touches a Raydium AMM/CP pool fires a
`tokio::spawn` with a move closure. Spawning costs ~1-5us but more importantly
each task holds a `reqwest::Client` + `Arc<StateCache>` + `Arc<Semaphore>` —
the semaphore caps concurrency at 10 so extra spawns just queue.
**Fix:** Replace with a `tokio::sync::mpsc` worker: a small pool of 4 long-
lived workers reading fetch jobs from a bounded channel. Eliminates per-event
spawn overhead + avoids unbounded future queue when semaphore is saturated.
Also move `Pubkey::to_string` allocations (line 780,889,1017) out of the JSON
payload by using pre-computed base58 strings cached per pool.
**Estimated delta:** ~5-15us/event on Raydium paths; reduced tail latency.

### 12. Dropping `blocking` spawn uses full tokio rt handle
**File:** `src/main.rs:270`
**Cost:** `tokio::task::spawn_blocking` moves the router into a tokio blocking
pool thread (default cap 512). Router is the single sync hot-path — it should
be a plain `std::thread::Builder` with pinned CPU affinity on Frankfurt.
Eliminates blocking-pool scheduling jitter.
**Fix:** `std::thread::Builder::new().name("router").spawn(...)` + optional
`core_affinity` to pin to an isolated core. Pass the channel + tokio `Handle`
the same way.
**Estimated delta:** cleanup / removes scheduler jitter spikes (tail latency).

### 13. `GeyserStream` sends `PoolStateChange` only when both mints are cached
**File:** `src/mempool/stream.rs:672-678`
**Cost:** First-ever event for a new pool pair is silently dropped and the
second event arrives ~400ms later. That is a lost opportunity window on brand-
new pools. Not a latency issue but a coverage gap.
**Fix:** Forward the event anyway and let the bundle builder treat missing
mint programs as a retryable error (current behavior — `anyhow::anyhow!("Mint
program unknown …")`). Router currently discards routes with unknown mints via
the ATA resolution — first submission after cache populate works.
**Estimated delta:** cleanup / marginal coverage gain.

### 14. Tracing JSON layer on hot path
**File:** `src/main.rs:44-47`
**Cost:** `fmt::layer().json()` serializes each log event into JSON. The
`OPPORTUNITY #…` info log fires per opportunity with 10+ fields — the
`serde_json` serialization + stdout write is ~50-150us. `env_filter`
already allows `debug` for `solana_mev_bot` which amplifies per-event logs.
**Fix:** On the hot path use the non-JSON compact formatter
(`fmt::layer().compact()`) or gate log verbosity behind an env var. Move the
`info!("SUBMITTED bundle #{}: …")` (main.rs:572) and `OPPORTUNITY` log to
`debug!` when not in verbose mode; export the numeric data via metrics instead
(already partly done).
**Estimated delta:** 50-150us per opportunity (the `info!` fires after the
dispatch so doesn't block submit, but it does delay the next event).

---

## Not pursued (either DONE or speculative)

- Prebuilt Pubkey constants — already done via `addresses.rs`.
- ALT expansion / per-pool ALT — already in remaining roadmap.
- Removing Phoenix lot-size quoting — already in remaining roadmap.
- Switching relays to gRPC — deferred, needs cooperating relay support.
