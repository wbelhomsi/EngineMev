# EngineMev Leverageability Audit — 2026-04-19

Audit of `/home/lunatic/projects/EngineMev/` vs. its six binaries. Totals: 13,379 lines across `src/`; binary entries are `main.rs` (663) plus five in `bin/` (464/359/499/337/827). Everything else is library code.

## Executive Summary

**Cleanest 20%:** `src/mempool/parsers/*` (10 files, ~850 lines), `src/router/dex/*` (9 files, ~1100 lines), `src/executor/relays/*` (~1470 lines), `src/addresses.rs`. Pure, DEX-specific building blocks with no strategy imprint. `parsers` is the only module imported by 4/6 binaries — the clearest reuse win.

**Most tangled 20%:**
1. `src/cexdex/*` mixes strategy-specific code (`detector.rs`, `simulator.rs`, `stats.rs`) with infra everyone wants (`price_store.rs`, `nonce.rs`). Three non-cexdex clients reach into `crate::cexdex` (`feed/binance.rs:13`, `bin/manifest_mm.rs:36`, `mempool/stream.rs:89`) — the module name is lying.
2. `src/main.rs` (663) and `src/bin/cexdex.rs` (827) each reimplement startup: tracing, ctrl-c, blockhash/tip-floor spawn, keypair load, relay construction. ~350 lines of drift-prone copy-paste.
3. `src/executor/bundle.rs:318-411` hardcodes `match DexType` for 10 variants. Adding an 11th DEX requires editing this file; there's no trait.

---

## Reusable gems (keep these, formalize as stable API)

| Module | File(s) | Clients (binaries) | Why it's good |
|---|---|---|---|
| Per-DEX Geyser parsers | `src/mempool/parsers/*.rs` | 4/6 bins (`main`, `cexdex`, `manifest_mm`, `xstocks_probe`, `manifest_discover`) | Pure `fn parse_*(pk, data, slot) -> Option<PoolState>`, no strategy state. Directly used by `xstocks_probe.rs:31-33` and `manifest_discover.rs:31`. |
| Relay primitives | `src/executor/relays/common.rs` (705), + 5 relays | 2/6 (`main`, `cexdex`) | `Relay` trait + `RateLimiter` + `build_signed_bundle_tx` (multi-ALT) + `parse_jsonrpc_response` are generic. Both submitting binaries use identical plumbing. |
| `RelayDispatcher` + `BundleBuilder` | `src/executor/{relay_dispatcher,bundle}.rs` | 2/6 | Correctly shared. `cexdex::bundle::build_instructions_for_cex_dex` at `src/cexdex/bundle.rs:14-41` proves the shape works for 1-hop. |
| `StateCache` / `BlockhashCache` / `TipFloorCache` | `src/state/*.rs` | 2/6 directly, 3/6 transitively | Lock-free DashMap, background refreshers. |
| `feed::binance::run_solusdc_loop` | `src/feed/binance.rs:29` | 2/6 (`cexdex`, `manifest_mm`) | Auto-reconnect, shutdown-aware. **But** coupled to `cexdex::PriceStore` (see smells). |
| `addresses.rs` | `src/addresses.rs` | All binaries transitively | Compile-time const `Pubkey`s, zero runtime cost. |
| Per-DEX swap IX builders | `src/executor/swaps/*.rs` | `main` via `BundleBuilder`, `manifest_mm` directly (`bin/manifest_mm.rs:37`) | Uniform signature, easy to extend. |

---

## Duplicated code

| Concept | Locations | Lines each | Proposed consolidation |
|---|---|---|---|
| Keypair loading | `src/rpc_helpers.rs:12-33` + `src/bin/setup_alt.rs:179-200` | 22 / 22 | Call `rpc_helpers::load_keypair` from `setup_alt`. The duplicate exists only because `setup_alt` doesn't import the crate today. |
| Raw `getAccountInfo` / `getMultipleAccounts` | `mempool/stream.rs:698-798`, `executor/confirmation.rs:232-290`, `state/blockhash.rs:62`, `bin/xstocks_probe.rs:164`, `bin/manifest_mm.rs:419`, `bin/manifest_discover.rs:264` | ~30 × 6 | Extract `rpc::get_account` / `rpc::get_multiple_accounts` helpers in `rpc_helpers.rs`. Saves ~150 lines, centralizes error handling. |
| Tracing + ctrl-c + dotenv | `main.rs:37-67+140`, `bin/cexdex.rs:29+91`, `bin/manifest_mm.rs:85+216`, `bin/xstocks_probe.rs:72+127`, `bin/manifest_discover.rs:86`, `bin/setup_alt.rs:363` | 10-40 × 6 | Add `harness::setup_tracing(name, otlp)` + `harness::shutdown_watch()` returning a pre-wired `watch::Receiver<bool>`. |
| Auto-shutdown timer (`*_RUN_SECS`) | `bin/cexdex.rs:98-110`, `bin/manifest_mm.rs:206-212+229-235` | 12 × 2+ | Fold into `shutdown_watch()` with a `RUN_SECS` argument. |
| CEX-DEX bundle adapter | `cexdex/bundle.rs:14-41` | 40 | Promote to `executor::bundle::build_single_hop` — nothing about it is cexdex-specific. |

---

## Coupling smells

1. **`feed/binance.rs:13` imports `crate::cexdex::PriceStore`.** A Binance feed should not know about cexdex. `PriceStore` is a `DashMap<Symbol, PriceSnapshot>` + pool snapshots — belongs in `feed::` or `state::price_store`. This is why `bin/manifest_mm.rs:36` writes `use solana_mev_bot::cexdex::price_store::PriceStore;` for a market-making binary that has nothing to do with CEX-DEX arb.

2. **`mempool/stream.rs:89` declares `nonce_pool: Option<crate::cexdex::NoncePool>`.** Geyser is strategy-agnostic infra; this makes every future strategy depend on `cexdex` for durable-nonce fan-out. `NoncePool` + `NonceInfo` at `src/cexdex/nonce.rs:1-60` are pure plumbing — move to `state::nonce` next to `BlockhashCache`.

3. **`cexdex/config.rs:10` imports `router::pool::DexType`.** `CexDexConfig.pools: Vec<(DexType, Pubkey)>` at line 29 forces the DEX enum to stay stable for config compatibility — deserialize to a string tag at the boundary instead.

4. **`cexdex/geyser.rs:28-60` fakes a `BotConfig` with empty fields** (relay endpoints, keypair path, tip fractions all blanked). 60-line workaround for `GeyserStream::new` demanding `Arc<BotConfig>` at `mempool/stream.rs:96`. The stream only reads 5 fields. Extract a `GeyserConfig` struct + `BotConfig: From<&GeyserConfig>`.

5. **`executor/bundle.rs:318-411`: hand-written `match DexType` over 10 variants.** Every swap builder has the same shape. Formalize as `trait SwapBuilder` in `executor/swaps/mod.rs` with a `HashMap<DexType, Box<dyn SwapBuilder>>` registry.

6. **`main.rs:188-233` and `bin/cexdex.rs:189-276`: copy-pasted relay wiring** (~45 lines each). Extract `RelayDispatcher::from_config(config, signer, alts, relay_mask)`.

7. **`cexdex/mod.rs` re-exports 5 names**; 3 (`PriceStore`, `NoncePool`, `Inventory` in spirit) are strategy-agnostic. Post-refactor the public API should be ~2 names.

---

## Abstractions worth formalizing

| Implicit contract today | Explicit form proposed |
|---|---|
| Parser shape enforced by convention in `mempool/parsers/*` | `trait PoolParser { fn parse(...); fn accepts_size(len: usize) -> bool; }` with a `&[&dyn PoolParser]` registry. Drops the data-size `if/else` chain in `stream.rs`. |
| Swap-builder shape enforced by `build_*_swap_ix` naming | `trait SwapBuilder` + `HashMap<DexType, Box<dyn SwapBuilder>>` on `BundleBuilder`. Kills the 94-line match in `bundle.rs:318-411`. |
| `run_solusdc_loop` hardcodes SOL/USDC | `trait CexFeed { async fn run(store, shutdown); }` + `BinanceFeed::new("SOLUSDC")`. Needed the moment `manifest_mm` quotes non-SOL (warns today at `bin/manifest_mm.rs:96-100`). |
| "Every strategy wires tracing + ctrl-c + blockhash + tip-floor + dispatcher" — copy-pasted | `struct StrategyHarness` in `src/harness.rs` owns the watch channel, spawns refreshers, returns `(BlockhashCache, TipFloorCache, Option<RelayDispatcher>, watch::Receiver<bool>)`. Collapses `main.rs` + `bin/cexdex.rs` startup from ~180 lines each to ~30. |
| `BotConfig`, `CexDexConfig`, `MmConfig` all parse env independently | `trait FromEnv` + a `BaseEngineConfig` (geyser/rpc/keypair) that strategy configs compose. Already ~70% there. |

---

## Bottom line

The bottom of the stack (parsers, relays, addresses, DEX math) is excellent — pure, testable, DEX-additive. The top (`cexdex`, `main.rs`, binary startup) has absorbed too much. One `harness` module + two trait refactors (`SwapBuilder`, `PoolParser`) + three module moves (`PriceStore` to `feed::`, `NoncePool` to `state::`, `Inventory` stays in cexdex) would make adding a 7th binary a ~100-line exercise instead of ~500.

Priority if time limited:
1. Move `PriceStore` out of `cexdex/` (1h — unblocks any feed-consuming strategy).
2. Move `NoncePool` out of `cexdex/` (2h — unblocks fan-out for main engine too).
3. Extract `StrategyHarness` (1d — eliminates the biggest source of drift).
4. `SwapBuilder` trait refactor (1d — DEX additions stop touching `bundle.rs`).
