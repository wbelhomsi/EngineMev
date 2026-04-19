# EngineMev Waste Audit — 2026-04-19

## Executive Summary

Repo is ~17.6k LOC src + ~equivalent tests. Concrete deletion/cleanup candidates total **~2,900 src LOC and 6 crate dependencies**, dominated by three probes/scaffolds built in the last two weeks that never shipped and are not exercised in CI.

### Top 3 biggest wins

1. **Manifest MM scaffold** (`src/mm/`, `src/bin/manifest_mm.rs`, `src/bin/manifest_discover.rs`, `src/executor/swaps/manifest_mm.rs`) — 1,602 LOC across 6 files, wired as three separate bins + module tree. Dry-run only (see `src/bin/manifest_mm.rs:24` `MM_DRY_RUN=true` and `:250, :385` TODOs that gate real submission). No orchestrator, no fill detection, no inventory sync.
2. **xstocks probe** (`src/bin/xstocks_probe.rs`, 359 LOC) — one-shot JSONL price logger, never integrated into engine, not referenced outside itself.
3. **Unused deps + `state/bootstrap.rs`** — `cargo machete` flags 6 unused crates (see Deps table); `src/state/bootstrap.rs` is a 5-line comment stub that replaces a deleted function (`Grep "state::bootstrap"` returns zero src hits).

---

## 1. Unused code

| file:line | What | Evidence |
|---|---|---|
| `src/state/bootstrap.rs:1-5` | Entire file: 5-line comment-only orphan. | `Grep "state::bootstrap"` returns 0 hits in src; 3 hits in old plan docs. `state/mod.rs:1-7` doesn't `pub mod bootstrap`. |
| `src/router/calculator.rs:51-54` (`find_routes`) + `src/router/pool.rs:278-292` (`DetectedSwap`) | Legacy `find_routes(&DetectedSwap)` + the struct it takes. | `Grep "find_routes\b"` in src shows only the def at `calculator.rs:51`. `Grep DetectedSwap` src: only its def in `pool.rs` and an import in `calculator.rs`. All usages are in `tests/`. `src/main.rs:14` comment explicitly says "DetectedSwap no longer used". |
| `src/metrics/counters.rs:19-21` (`inc_geyser_reconnections`) | Defined, never called. | `grep inc_geyser_reconnections src/ tests/` = 0 hits outside `counters.rs`. (LaserStream reconnects internally and does not surface an event.) |
| `src/config.rs:182` (`jito_auth_keypair_path` field) + `:262` (`JITO_AUTH_KEYPAIR` env read) | Struct field populated from env, never read. | `Grep jito_auth_keypair_path` src: only the struct def + 2 stub assignments (`src/cexdex/geyser.rs:39`, `src/bin/cexdex.rs:226`) passing `String::new()`. No relay reads it — Jito auth is the `JITO_AUTH_UUID` header in `executor/relays/jito.rs`. Also in `.env.example:19`. |
| `src/mm/config.rs:11,13,14,61,50-51,129` (5 fields) | `searcher_private_key`, `geyser_grpc_url`, `geyser_auth_token`, `metrics_port`, `requote_threshold_frac` parsed but never read. | `Grep` of each against `src/bin/manifest_mm.rs` + `src/mm/*.rs` returns 0 hits. No metrics server is started; no Geyser is wired; `searcher_private_key` is never signed with (binary is dry-run). |
| `src/bin/manifest_mm.rs` TODOs at `:250, :385` + stub at `:97` | "wire real inventory when seat sync is built"; "real submission path". | `Grep "TODO\|stub"` shows 3 hits in this file — the bin is a scaffold, not a product. |
| `src/bin/setup_alt.rs` (464 LOC) | One-shot CLI, builds/extends an ALT. | Not declared as a `[[bin]]` in `Cargo.toml:101-120` (only `cexdex`, `solana-mev-bot`, `xstocks_probe`, `manifest_mm`, `manifest_discover` are). Cargo's auto-bin discovery still builds it; useful once per ALT change. Not run since ALT last updated. **Keep for now; see §"Keep but rename/document"**. |

**Estimated deletion: ~1,700 LOC** (mm module + manifest_mm binary + DetectedSwap/find_routes + bootstrap.rs + metric fn + config field).

---

## 2. Unused dependencies

Running `cargo machete` from a clean state (confirmed by direct `Grep` of each):

| Crate | Cargo.toml:line | Evidence of non-use |
|---|---|---|
| `bytemuck` | 41 | `Grep -r "bytemuck\|use bytemuck"` in src: 0 hits. Comment at `Cargo.toml:38-40` references a historical use case (Phoenix/Manifest raw parsing) — actual parsers use manual `from_le_bytes`. |
| `five8_core` | 14 | 0 hits. Comment at `:13` says "workaround for upstream keypair bug" — upstream since fixed, no longer needed. |
| `opentelemetry-semantic-conventions` | 70 | 0 hits. `tracing_layer.rs` imports only `opentelemetry`, `opentelemetry_otlp`, `opentelemetry_sdk`, `tracing_opentelemetry`. |
| `thiserror` | 59 | 0 hits (`use thiserror` or `thiserror::` — zero). |
| `tokio-stream` | 25 | 0 hits (`tokio_stream`). We use `futures::StreamExt`. |
| `tonic` | 23 | 0 hits. `helius-laserstream` embeds tonic privately. |

**Estimated saving: 6 crates removed → shorter dep tree, faster cold builds.** (Comment block `Cargo.toml:37-41` should also go.)

---

## 3. Unused / stale env vars

| Var | Declared at | Status |
|---|---|---|
| `JITO_AUTH_KEYPAIR` | `config.rs:262`, `.env.example:19` | Read into `jito_auth_keypair_path`, never consumed. |
| `MM_SEARCHER_PRIVATE_KEY`, `MM_METRICS_PORT`, `MM_REQUOTE_THRESHOLD_FRAC` | `mm/config.rs:67-68, 117, 129` | Parsed, stored on `MmConfig`, never referenced in `src/bin/manifest_mm.rs`. |
| `CEXDEX_HARD_CAP_RATIO`, `CEXDEX_SKEWED_PROFIT_MULTIPLIER` | `cexdex/config.rs:141,147` | Each has 1 callsite — passed into `Inventory::new()`. OK, but flagged in the CLAUDE.md first-hour run as "untuned, 0 impact yet". Keep. |
| `CEXDEX_TIP_FRACTION_NOZOMI`, `..._BLOXROUTE`, `..._ZEROSLOT` | `cexdex/config.rs:175-177` | Parsed, but `src/bin/cexdex.rs:265-268` only constructs Jito + Astralane relays. The other three will never fire; the map entries are dead weight. |

---

## 4. Redundant / speculative RPC calls

- `src/bin/cexdex.rs:285-306` — periodic 30s balance refresh via `getBalance`/`getTokenAccount` even when nothing trades. In 16h the binary produced 5 opportunities; balance refresh ran ~1920 times. Change to on-demand (after every landing) or back off to 300s until realized PnL > 0.
- `src/executor/confirmation.rs:223-318` (`check_competitor`) fires `getSignaturesForAddress` + `getTransaction` for every dropped bundle. Main engine drops >100/hr; these calls are informational-only and CLAUDE.md:pitfall 28 notes `getBundleStatuses` is rate-limited. This extra pair of calls amplifies throttle risk.

---

## 5. Over-eager computation / fields nobody reads

- `src/router/pool.rs:100-106` — `PoolExtra.is_mayhem_mode`, `is_cashback_coin` populated by PumpSwap parser but never read by `executor/swaps/pumpswap.rs`. `Grep is_mayhem_mode src/ tests/` shows only writes in `parsers/pumpswap.rs` and the struct def.
- `src/router/pool.rs:132-135` — `best_bid_price`, `best_ask_price` populated only for Phoenix/Manifest but CLAUDE.md:pitfall 21 says Phoenix full-book parsing is deferred → these are always `None`. The Manifest parser does fill them; only `dex/manifest.rs` quote reads them. OK for Manifest; dead for Phoenix.

---

## 6. Log spam

- `src/main.rs:462-479` `info!("OPPORTUNITY #{}: ...")` fires every time the simulator returns Profitable. In a high-traffic slot this is 5-10/s, producing multi-KB JSON lines. Main engine has 0 landings → every one is noise. Consider demoting to `debug!` until something lands, or sampling 1-in-N.
- `src/main.rs:647` `info!("Cache: {} pools tracked")` every 30s, always identical once warmed. Demote to `debug!`.
- `src/mempool/stream.rs:218` post-subscribe `info!` is fine (startup-only). No issue there.

---

## 7. Dead / under-consumed metric counters

Only the cexdex dashboard is provisioned (`monitoring/provisioning/dashboards/cexdex-pnl.json`). Main-engine metrics are all emitted but none have a panel:

| Counter/histogram | Incremented? | Dashboard? |
|---|---|---|
| `inc_geyser_reconnections` | **No** (`Grep` = 0) | n/a |
| `geyser_updates_total`, `geyser_parse_errors_total`, `geyser_parse_duration_us` | Yes | **No panel** |
| `vault_fetches_total`, `vault_fetch_errors_total` | Yes | **No panel** |
| `routes_found_total`, `route_calc_duration_us`, `simulation_duration_us`, `pipeline_duration_us` | Yes | **No panel** |
| `bundles_submitted_total`, `bundles_landed_total`, `bundles_dropped_total`, `landed_estimated_profit_lamports_total`, `landed_estimated_tips_lamports_total` | Yes | **No panel** |
| `cache_pools_tracked`, `channel_backpressure`, `blockhash_age_ms`, `geyser_lag_slots` | Yes | **No panel** |

**Either build a main-engine dashboard or remove the metric calls.** ~21 incremented counters have zero consumer.

---

## 8. Stub/placeholder code older than 30 days — none

All TODOs are in `manifest_mm.rs`, from 2026-04-18 (3 days old). Under the 30-day threshold.

---

## 9. Lying / misleading comments

| file:line | Comment says | Reality |
|---|---|---|
| `src/router/simulator.rs:163` | "Sanity cap: any single arb showing > 1 SOL net profit is almost" (sentence cut) | Cap is 10 SOL (`:167` const `MAX_SANE_PROFIT = 10_000_000_000`). Second comment at `:164` correctly says 10 SOL. First line is a stale leftover. |
| `src/main.rs:401` | `format!("sanity cap: estimated profit {} > 1 SOL", ...)` | Same bug — threshold is 10 SOL. |
| `src/state/cache.rs:37` | "Keep TTL tight — 1 slot (~400ms) is ideal." | Actual TTL is 2s (`config.rs:284`). |
| `CLAUDE.md:60` | "Route by data size (653=Orca, 1560=CLMM, 904=DLMM, 1112=DAMM v2, 752=Raydium AMM, 637=Raydium CP)" | Missing PumpSwap (10th DEX), Phoenix + Manifest handled by `try_parse_orderbook`. Mostly correct, just incomplete. |
| `src/mempool/stream.rs:43-46` | "1. Subscribe to token vault accounts owned by target DEX programs / 2. When vault balances change" | We subscribe to **pool-state** accounts, not vaults (CLAUDE.md pitfall 4). The comment inverts the architecture. |

---

## Keep but rename / document

- `src/bin/setup_alt.rs` — operational tool, not part of the hot path. Cargo auto-discovers it but it's absent from `Cargo.toml`'s explicit `[[bin]]` list. Either add `name = "setup_alt"` to be explicit, or move to `scripts/` and convert to a Makefile target.
- `src/bin/xstocks_probe.rs` — research probe. If still needed for the xStocks hypothesis, move to `src/bin/probes/` and add a README. Otherwise delete.
- `src/mm/` + `src/bin/manifest_mm.rs` + `src/bin/manifest_discover.rs` + `src/executor/swaps/manifest_mm.rs` (1,602 LOC) — market-making scaffold built 3 days ago, still dry-run (`:97, :250, :385` stubs). If MM is frozen pending cex-dex validation, move under `experiments/mm/` or a feature flag (`--features mm`) to keep it out of the main dep graph. Otherwise ship it or delete.
- `src/router/calculator.rs:51 find_routes` — if kept for tests, mark `#[cfg(any(test, feature="legacy-api"))]` so it's obvious it's test-only.

---

## Safety warnings — looks unused but isn't

- **OTLP deps (`opentelemetry*`, `tracing-opentelemetry`)** — look dormant; enabled only when `OTLP_ENDPOINT` is set. Keep.
- **`tokio-tungstenite`** — no top-level `use` in src, but `state/tip_floor.rs` and `feed/binance.rs` do `use tokio_tungstenite::...` (confirmed via `Grep`). Keep.
- **All 10 DEX swap/parser modules** — each one is referenced from `executor/swaps/mod.rs` and `mempool/parsers/mod.rs`, wired via `can_submit_route` at `router/calculator.rs:353`. Even Phoenix (deferred for submission per pitfall 21) is still on the allowlist — deleting it would also require removing it from `DexType` enum plus 20+ downstream matches.
- **`src/bin/setup_alt.rs`** — looks orphan (no `[[bin]]` in Cargo.toml) but `cargo build --bin setup_alt` succeeds via auto-discovery. Don't delete without first confirming no deployment doc references it.
- **`nonce_accounts` / `CEXDEX_SEARCHER_NONCE_ACCOUNTS`** — empty by default; when empty, cexdex runs single-relay-Jito. The field and code path look dead in dry-run but is load-bearing for the multi-relay fan-out path (`cexdex/nonce.rs` + 4 metrics). Keep.
- **`solana-mev-bot` bin in `Cargo.toml:106`** — the main engine binary, don't rename to match the CLAUDE.md "EngineMev" branding without checking deploy scripts.

---

## Rough totals

| Category | LOC / items | Action |
|---|---|---|
| Dead files + fields (bootstrap.rs, DetectedSwap, find_routes, unused metric fn, `jito_auth_keypair_path`) | ~120 LOC | delete |
| Manifest MM scaffold | ~1,600 LOC | gate behind feature or delete |
| xstocks probe | ~360 LOC | move to scripts/ or delete |
| setup_alt.rs | 464 LOC | keep, add explicit `[[bin]]` entry |
| Unused deps | 6 crates | remove from Cargo.toml |
| Under-consumed main-engine metrics | ~21 counters | add panel or remove calls |
| Misleading comments | 5 spots | fix inline |
