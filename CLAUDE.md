# EngineMev — Solana MEV Backrun Arbitrage Engine

## Git Co-Author

When committing, always use this co-author line (not the claude-flow default):
```
Co-Authored-By: Claude <noreply@anthropic.com>
```

## What This Is

Halal-compliant MEV backrun engine on Solana. Detects price dislocations across 8 DEXes (6 AMMs + 2 CLOBs) via Yellowstone Geyser (Helius LaserStream) account streaming, then submits atomic arbitrage bundles via multi-relay fan-out (Jito, Nozomi, bloXroute, Astralane, ZeroSlot).

**Repo:** github.com/wbelhomsi/EngineMev
**Status:** LIVE on co-located Frankfurt server. Submitting bundles via Jito + Astralane. Pipeline latency p50=906us (optimized from 15ms). No profitable arb landed yet — competing against sub-ms co-located bots.

## Halal Compliance — Non-Negotiable

All strategies MUST be Halal. This is a hard constraint, not a preference.

- **Allowed:** Spot arbitrage, JIT liquidity provision on spot DEX pools, intent solving
- **Forbidden:** Riba (interest/usury), debt exploitation (no liquidation bots), maysir (gambling/token sniping), sandwich attacks, any lending protocol interaction, leveraged positions
- Never suggest or build anything that touches lending, borrowing, or liquidation

## Architecture

Post-mempool design (Jito mempool was killed March 2024):

```
Yellowstone Geyser → pool state account change → per-DEX parser → update StateCache
  → detect price dislocation → find arb route → simulate profit
  → build bundle (arb tx + Jito tip) → multi-relay fan-out → next slot
```

This is NOT same-block backrunning. We observe state changes post-block and submit for the next slot.

### Geyser Subscription Strategy

Two modes via `SubscriptionMode` (in `mempool::stream`):

**Main engine — `WideByOwner` (default)**: subscribe by DEX program owner across all 9 DEXes + LST stake pool accounts. Required for lazy pool discovery.
- Geyser streams pool state account updates when swaps happen
- Per-DEX parsers extract reserves/pricing from pool-specific layouts
- Category A (Orca, CLMM, DLMM, DAMM v2): reserves derived from pool state directly
- Category B (Raydium AMM v4, CP): lazy vault balance fetch via `getMultipleAccounts` per swap event
- Zero-bootstrap: all pools discovered lazily via Geyser (no getProgramAccounts at startup)

**CEX-DEX binary — `SpecificAccounts(pools)`**: subscribe only to the configured `CEXDEX_POOLS` pubkeys. The cexdex binary monitors a fixed, small pool set (typically 4–8 SOL/USDC pools); the wide owner-based subscription would stream every pool change across 9 mainnet DEX programs and fire unnecessary RPC fetches. Narrow mode eliminates that overhead.

NEVER subscribe to Token Program via Geyser — would receive every token transfer on Solana (millions/sec).

## Key Technical Decisions

- **No jito-sdk-rust dependency**: Raw JSON-RPC via reqwest is leaner.
- **No Jito gRPC SearcherServiceClient**: Deprecated since March 2024.
- **Helius LaserStream SDK** (`helius-laserstream 0.1.9`): Streams pool state changes from validator memory at sub-50ms. Built-in auto-reconnection with slot-based replay, Zstd compression (70-80% bandwidth reduction), TLS. Replaces manual `yellowstone-grpc-client` connection + reconnection logic.
- **solana-sdk 4.0.1 + modular crates**: `solana-system-interface` (with `bincode` feature) for system instructions, `solana-message` for `AddressLookupTableAccount`, `solana-address-lookup-table-interface` for ALT deserialization. `five8_core` with `std` feature as workaround for upstream keypair bug.
- **crossbeam-channel** between async Geyser stream and sync router thread.
- **DashMap** for lock-free concurrent cache reads across threads.
- **Per-DEX parsers in stream.rs**: Route by data size (653=Orca, 1560=CLMM, 904=DLMM, 1112=DAMM v2, 752=Raydium AMM, 637=Raydium CP). Phoenix and Manifest use variable-size accounts routed by `try_parse_orderbook()` fallback instead of data size.
- **BlockhashCache**: `Arc<RwLock>` with 5s staleness, background 2s refresh via `getLatestBlockhash`.
- **Jito tip floor via WebSocket**: `wss://bundles.jito.wtf/api/v1/bundles/tip_stream` pushes real-time tip data (SOL floats, converted to lamports). Falls back to REST polling. Replaces 5s REST polling.
- **Slippage-tolerant profit model**: `SLIPPAGE_TOLERANCE` env (default 0.25). Gross profit discounted by 25% before calculating tip and `min_final_output`. On-chain arb-guard enforces the slippage-adjusted minimum.
- **Route calculator optimizations** (see "Tunable Constraints" below): pool cap per token, early exit, single SOL-base search, `get_any()` skips TTL in route discovery. Reduced pipeline from 15ms to <1ms.
- **API key redaction**: `config::redact_url()` strips keys from all log output.

## Module Map

```
src/
├── main.rs              # Main engine entry: Geyser → Router → Bundle → Relay
├── bin/
│   └── cexdex.rs        # CEX-DEX arb binary (Binance SOL/USDC, Model A inventory-based)
├── lib.rs               # Re-exports modules for integration tests
├── addresses.rs         # Centralized const Pubkey for all program IDs, mints (compile-time, zero runtime cost)
├── config.rs            # Env config, relay endpoints, redact_url()
├── sanctum.rs           # Sanctum bootstrap: virtual pools, LST indices, rates, update_virtual_pool
├── rpc_helpers.rs       # load_keypair, load_alt, simulate_bundle_tx, send_public_tx
├── feed/                # CEX price feeds (strategy-neutral infra)
│   ├── mod.rs           # PriceSnapshot, PriceStore (shared CEX snapshots + pool StateCache)
│   ├── binance.rs       # Binance bookTicker WS with auto-reconnect
│   └── price_store.rs   # Shared across cexdex, manifest_mm, and any future CEX-consuming strategy
├── cexdex/              # CEX-DEX arbitrage strategy
│   ├── mod.rs           # Re-exports CexDexConfig, Inventory, ArbDirection, CexDexRoute
│   ├── config.rs        # CEXDEX_* env var parsing
│   ├── units.rs         # Decimal conversions (SOL lamports, USDC atoms, bps)
│   ├── inventory.rs     # Balance tracking, ratio gates, reservation lifecycle
│   ├── route.rs         # CexDexRoute + ArbDirection
│   ├── detector.rs      # Divergence detection, trade sizing (pool-depth-bounded)
│   ├── simulator.rs     # CEX-priced profit sim, tip calculation, min_final_output
│   ├── bundle.rs        # Adapter: CexDexRoute → ArbRoute → BundleBuilder
│   └── geyser.rs        # Narrow Geyser wrapper for cexdex binary
├── mempool/
│   ├── mod.rs           # Exports GeyserStream, PoolStateChange
│   ├── stream.rs        # LaserStream gRPC subscription, data-size routing,
│   │                    # lazy vault/bin-array/tick-array fetches
│   └── parsers/         # Per-DEX Geyser pool state parsers
│       ├── mod.rs       # Shared: approx_reserves_from_sqrt_price, re-exports
│       ├── orca.rs, raydium_amm.rs, raydium_cp.rs, raydium_clmm.rs
│       ├── meteora_dlmm.rs, meteora_damm_v2.rs
│       ├── phoenix.rs, manifest.rs, pumpswap.rs
│       └── (each file: one parse_*() function)
├── router/
│   ├── mod.rs           # Exports RouteCalculator, ProfitSimulator, can_submit_route
│   ├── pool.rs          # DexType, PoolState (types + dispatcher), ArbRoute, RouteHop
│   ├── calculator.rs    # 2-hop and 3-hop route discovery, pool cap, early exit
│   ├── simulator.rs     # Go/no-go gate: slippage-adjusted tips, min_final_output
│   └── dex/             # Per-DEX quoting math
│       ├── mod.rs       # Shared: ceil_div, compute_swap_step, tick_to_sqrt_price, clmm_single_tick
│       ├── cpmm.rs      # Constant product (Raydium AMM/CP, PumpSwap, DAMM v2 flat)
│       ├── clmm_orca.rs, clmm_raydium.rs  # Multi-tick crossing
│       ├── dlmm.rs      # Meteora DLMM bin-by-bin
│       ├── damm_v2.rs, sanctum.rs
│       └── phoenix.rs, manifest.rs  # Orderbook quoting
├── executor/
│   ├── mod.rs           # Exports BundleBuilder, RelayDispatcher
│   ├── bundle.rs        # BundleBuilder, execute_arb_v2 CPI, ATA/wSOL logic
│   ├── swaps/           # Per-DEX swap instruction builders
│   │   ├── mod.rs       # Re-exports + shared floor_div
│   │   ├── raydium_amm.rs, raydium_cp.rs, raydium_clmm.rs
│   │   ├── orca.rs, meteora_dlmm.rs, meteora_damm_v2.rs
│   │   ├── sanctum.rs, phoenix.rs, manifest.rs, pumpswap.rs
│   │   └── (each file: one build_*_swap_ix() function)
│   ├── confirmation.rs  # Bundle confirmation tracker + competitor analysis
│   ├── relay_dispatcher.rs  # Concurrent relay fan-out with ALT support
│   └── relays/
│       ├── mod.rs       # BundleRelay trait, RelayResult
│       ├── common.rs    # Shared: RateLimiter, build_signed_bundle_tx, parse_jsonrpc_response
│       ├── jito.rs      # Jito block engine relay
│       ├── nozomi.rs    # Nozomi relay
│       ├── bloxroute.rs # bloXroute relay
│       ├── astralane.rs # Astralane relay (HTTP/2 keepalive)
│       └── zeroslot.rs  # ZeroSlot relay
├── metrics/
│   ├── mod.rs           # init(), shutdown(), Prometheus HTTP server
│   ├── counters.rs      # All metric helper functions (atomic, zero-cost)
│   └── tracing_layer.rs # Optional OTLP tracing layer builder
└── state/
    ├── mod.rs           # Exports StateCache, BlockhashCache, TipFloorCache
    ├── cache.rs         # DashMap pool cache with TTL, token→pool index, bin/tick array caches
    ├── blockhash.rs     # BlockhashCache: Arc<RwLock>, 5s staleness, background 2s fetch loop
    └── tip_floor.rs     # TipFloorCache: Jito WebSocket stream, REST fallback
```

## DEX Program IDs (verified current)

| DEX | Program ID | Data Size | Anchor? |
|-----|-----------|-----------|---------|
| Raydium AMM v4 | `675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8` | 752 | No |
| Raydium CP (CPMM) | `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C` | 637 | Yes |
| Raydium CLMM | `CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK` | 1560 | Yes |
| Orca Whirlpool | `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc` | 653 | Yes |
| Meteora DLMM | `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo` | 904 | Yes |
| Meteora DAMM v2 | `cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG` | 1112 | Yes |
| Sanctum S Controller | `5ocnV1qiCgaQR8Jb8xWnVbApfaygJ8tNoZfgPwsgx9kx` | varies | Yes |
| Phoenix V1 | `PhoeNiXZ8ByJGLkxNfZRnkUfjvmuYqLR89jjFHGqdXY` | variable (624+ header) | No (Shank) |
| Manifest | `MNFSTqtC93rEfYHB6hF82sKdZpUDFWkViLByLd1k1Ms` | variable (256+ header) | No |
| PumpSwap AMM | `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA` | 243-301 (discriminator routed) | Yes |

**See `docs/DEX-REFERENCE.md` for full account layouts, byte offsets, and quoting math.**

## Jito Tip Accounts (8 total, rotated per bundle)

Hardcoded in `bundle.rs`. Dynamic fetch available via `getTipAccounts` JSON-RPC. Minimum tip: 1000 lamports. Auctions every 200ms. Tip floor REST: `https://bundles-api-rest.jito.wtf/api/v1/bundles/tip_floor`

## Build & Run

```bash
cargo check          # Verify compilation
cargo build --release # Release build (LTO fat, codegen-units=1)
cp .env.example .env  # Configure endpoints and keys
cargo run --release   # Starts in DRY_RUN=true by default
```

### Required .env configuration

```env
RPC_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_KEY
GEYSER_GRPC_URL=https://laserstream-mainnet-fra.helius-rpc.com  # LaserStream, NOT shared RPC
GEYSER_AUTH_TOKEN=YOUR_HELIUS_API_KEY
DRY_RUN=true
```

### Tests

```bash
make test                                     # 242 unit tests
make lint                                     # clippy (warnings = errors)
make coverage                                 # line coverage report (49.3%)
make ci                                       # lint + test + coverage
cargo test --features e2e --test e2e          # 4 e2e tests
cargo test --features e2e_surfpool --test e2e_surfpool  # Surfpool E2E (needs RPC_URL + surfpool)
```

### CEX-DEX Binary

Separate binary for Binance SOL/USDC CEX-DEX arbitrage (Model A, inventory-based).
Uses a separate wallet for clean P&L isolation.

```bash
# First time: generate a separate searcher keypair
solana-keygen new -o cexdex-searcher.json

# Fund it with SOL + USDC (manual top-up)

# Set CEXDEX_POOLS to the specific pool addresses to monitor in .env

# Run in dry-run first
CEXDEX_DRY_RUN=true cargo run --release --bin cexdex

# Go live
CEXDEX_DRY_RUN=false cargo run --release --bin cexdex
```

See `docs/superpowers/specs/2026-04-16-cex-dex-arb-design.md` for the full design.

### CEX-DEX Multi-Relay Fan-Out (2026-04-17)

Safe fan-out to Jito + Astralane using durable Solana nonce accounts.
Every cexdex-submitted tx carries `advance_nonce_account(N, authority)` as
instruction #0 with the nonce's current hash in the `recent_blockhash`
slot. All relay copies of the same opportunity share the same nonce → only
one can land (the first to reach consensus advances the nonce; others
fail the nonce check atomically).

- Nonce pool: 3 accounts configured via `CEXDEX_SEARCHER_NONCE_ACCOUNTS`
- Round-robin selection by oldest `last_used` (config-order tiebreaker)
- Hash cache maintained by Geyser (extends the narrow-subscription filter
  to include nonce pubkeys); zero-RPC on hot path
- Startup RPC bootstrap validates authority == searcher wallet for each nonce
- Per-relay tip via `CEXDEX_TIP_FRACTION_JITO`, `CEXDEX_TIP_FRACTION_ASTRALANE`
- Simulator rejects if worst-case net (highest tip fraction) is unprofitable
- Metrics: `cexdex_nonce_collision_total`, `cexdex_nonce_in_flight`,
  `cexdex_bundles_attempted_total{relay}`, `cexdex_bundles_confirmed_total{relay}`,
  `cexdex_tip_paid_usd_micros_total{relay}`,
  `cexdex_detector_skip_total{reason}` (reasons: `global_cooldown`, `cex_stale`,
  `no_cex_snapshot`, `pool_not_cached`, `dedup_window`, `inventory_gate`,
  `try_route_none`, `below_min_profit_detector`, `not_sol_usdc_pool`,
  `zero_reserves`, `wrong_side_buy`, `wrong_side_sell`, `spread_too_tight`)

See `docs/superpowers/specs/2026-04-17-cexdex-nonce-fanout-design.md`.

### Manifest Market Discovery Binary

One-shot tool that enumerates every Manifest CLOB market via
`getProgramAccounts` (256-byte header dataSlice), cross-references against
a hardcoded halal mint allowlist (stablecoins + SOL + major LSTs:
jitoSOL, mSOL, bSOL, JupSOL, INF, bonkSOL), and for matches refetches
full data to surface live best bid/ask + vault depth.

```bash
RPC_URL=https://mainnet.helius-rpc.com/?api-key=...
cargo run --release --bin manifest_discover
# Output: stdout table + /tmp/manifest_markets.json
```

**First run (2026-04-19):** 1285 Manifest accounts → 25 halal-compatible
markets. Top MM candidates by depth: jitoSOL/SOL, mSOL/SOL, JupSOL/SOL
(all 1-bp spreads — already competitively market-made) and SOL/USDC
(20-bp spread but only $21K depth — we'd *be* the market). Stablecoin
pairs (PYUSD/USDC, USDT/USDC) have most depth but zero spread for MM
to capture. Full snapshot: `docs/manifest-markets-2026-04-19.md`.

## Critical Rules for Development

1. **ALWAYS web-search to verify any external API, SDK, or crate is current before using it.** We lost a full session building on the dead Jito mempool API. Training data goes stale.

2. **Prefer to fail than to send a losing transaction.** Every gate (simulator, minimum profit threshold) must default to rejection.

3. **Every millisecond matters.** Avoid unnecessary allocations on the hot path, keep the router sync, use pre-computed indices.

4. **Geyser streams pool state accounts, NOT token vaults.** Accounts owned by DEX programs are pool state (AmmInfo, Whirlpool, LbPair, etc.). SPL Token vaults are owned by Token Program. We parse pool state — see `stream.rs` per-DEX parsers.

5. **Never subscribe to Token Program via Geyser** — would receive every token transfer on Solana (millions/sec). Subscribe by DEX program owner instead.

6. **Raydium AMM v4 and CP don't store reserves in pool state.** Reserves live in SPL Token vault accounts. We do lazy vault fetch (`getMultipleAccounts` with `dataSlice: {offset: 64, length: 8}`) when pool state changes.

7. **API keys must never appear in logs.** Use `config::redact_url()` to strip keys before logging any URL or error message.

## Roadmap — Current Status

### Phase 1: EngineMev Core (SVM) — COMPLETE
Base DEX↔DEX backrun arb working live on mainnet.

**Done:**
- Geyser streaming with per-DEX pool state parsing (8 DEXes: 6 AMMs + 2 CLOBs)
- Lazy pool discovery via Geyser (zero bootstrap)
- Lazy vault fetch for Raydium AMM/CP
- CLMM single-tick math using u128 integer arithmetic (Orca, Raydium CLMM, DAMM v2 concentrated)
- CLMM fee rate uses 1,000,000 denominator (validated against production system)
- Profit sanity cap (1 SOL max) catches approximation artifacts from stale reserves
- Route calculator (2-hop and 3-hop)
- Profit simulator with fresh-state validation and fresh hop output writeback
- Bundle builder with minimum_amount_out enforcement and correct per-hop amount_in chaining
- Total tip accounting (Jito + Astralane) — simulator rejects if total tips >= profit
- Real swap IX builders for all 9 DEXes (Raydium AMM/CP/CLMM, Orca, DLMM, DAMM v2, Sanctum, Phoenix, Manifest)
- All 8 DEXes + Sanctum enabled in can_submit_route()
- Multi-relay fan-out (Jito/Nozomi/bloXroute/Astralane/ZeroSlot) with per-relay rate limiting
- Blockhash cache (2s refresh, 5s staleness)
- Helius LaserStream SDK with auto-reconnection, Zstd compression, slot-based replay
- API key redaction in all logs
- LST rate arb (Sanctum virtual pools, enabled for submission)
- Phoenix V1 + Manifest CLOB market parsing + swap IX builders (enabled for submission)
- Compile-time const Pubkeys in addresses.rs (zero runtime base58 parsing)
- Pre-computed pair index in StateCache for O(1) pool pair lookups
- Token-2022 support in Orca/CLMM/DAMM v2 IX builders (per-mint token program resolution)
- Per-pool fee parsing: Orca from pool state offset 45, Raydium AMM from tradeFee fields
- CLMM multi-tick crossing: walks initialized ticks with liquidity adjustment at boundaries
- DLMM bin-by-bin simulation: real bin liquidity with Q64.64 pre-stored prices
- Raydium AMM v4 SwapBaseInV2 (8-account IX, no Serum/OpenBook dependency)
- arb-guard Phase A: on-chain profit guard (start_check/profit_check with reentrancy lock)
- arb-guard Phase B: passthrough CPI executor for ALL DEXes (execute_arb_v2 with per-hop amount_in rewriting via balance diff)
- PumpSwap AMM integration (10th DEX): Pump.fun graduated tokens, 21-23 accounts, 125 bps conservative fee, cashback/volume tracking
- Shared relay common.rs: RateLimiter, build_signed_bundle_tx (multi-ALT), parse_jsonrpc_response
- Multiple ALT support: 56-address base ALT + competitor's 170-address ALT (226 unique addresses)
- RequestHeapFrame (256KB) in every transaction for complex CPI chains
- Decomposed main.rs: sanctum.rs, rpc_helpers.rs, can_submit_route in router
- Safety: TIP_FRACTION=0.50, slippage-adjusted tips, smart tip with dynamic Jito WS floor, sanity cap 10 SOL, i128 profit math, relay key redaction
- Optimized route calculator: get_any() (no TTL gate), pool cap per token (20), early exit (5 routes), 10 SOL minimum pool reserve filter
- Raydium AMM v4 SwapBaseInV2 (8 accounts, no Serum/OpenBook)
- Base58 encoding for Jito/Nozomi/ZeroSlot, base64 for Astralane/bloXroute
- Random tip account selection per bundle
- Route cap (10 per event) + min reserve filter (10 SOL)
- Prometheus + OTLP metrics with error categorization and profiling histograms
- 242 unit tests + 10 e2e + 1 integration, 0 clippy warnings
- Makefile: make lint, make test, make coverage, make ci
- 87% simulation success rate on mainnet, 177 bundles accepted per 5 min
- Monitoring: Prometheus + Grafana docker-compose in monitoring/ dir

**Remaining:**
- ~~Deploy arb-guard to mainnet~~ DONE
- ~~Upgrade solana-sdk 2.2 → modular crates 4.x~~ DONE
- ~~Grafana + OpenTelemetry metrics~~ DONE
- ~~Deduplication of repeated opportunities~~ DONE
- ~~Raydium AMM v4 Swap V2~~ DONE (8 accounts, no Serum)
- ~~Extend arb-guard CPI to all DEX types~~ DONE (execute_arb_v2 passthrough with hop chaining)
- ~~PumpSwap AMM integration~~ DONE (10th DEX)
- ~~Co-located server~~ DONE (Frankfurt, near Helius LaserStream FRA endpoint)
- ~~Pipeline latency optimization~~ DONE (15ms → <1ms p50)
- ~~Jito tip floor WebSocket~~ DONE (real-time tip stream)
- ~~Slippage-tolerant profit model~~ DONE (SLIPPAGE_TOLERANCE env)
- ~~Competitor analysis logging~~ DONE (pool/slot/signer/tip on dropped bundles)
- Phoenix lot size conversion (Phoenix excluded from submission for now)
- Dynamic per-pool ALTs for high-volume pools
- DEX module refactor (per-DEX files, see `docs/superpowers/plans/2026-04-16-dex-module-refactor.md`)

### Phase 3: CEX↔DEX Arb (cexdex binary) — v1 COMPLETE
Binance SOL/USDC bookTicker WS + narrow Geyser → divergence detector → CEX-priced simulator → bundle builder. Model A (inventory-based, single-leg on-chain swap, no CEX leg).

**Built:** `src/bin/cexdex.rs` + `src/cexdex/` (detector, simulator, inventory, stats) + `src/feed/binance.rs`. 30+ unit tests + 3 e2e tests. Stats collector writes `records.jsonl` + `summary.json` on shutdown.

**First 1h dry-run (2026-04-16):** 25 detections, 15 profitable, $0.25 gross total. Wallet was 100% SOL so only SellOnDex tested. Only 1 of 4 configured pools hit the 5 bps threshold. See `docs/CEXDEX-RUN-2026-04-16.md` for full analysis and recommended parameter changes.

**Safety gates (in `src/cexdex/simulator.rs`):**
1. Gross profit > 0
2. Net profit (after tip + fee) strictly positive — HARD FLOOR regardless of `CEXDEX_MIN_PROFIT_USD` config
3. Net profit >= `CEXDEX_MIN_PROFIT_USD` threshold
4. Belt-and-suspenders re-check in `src/bin/cexdex.rs` right before `dispatcher.dispatch(...)`
5. `CEXDEX_MIN_PROFIT_USD` must be strictly positive at config load

**Prometheus gauges for cexdex** (`CEXDEX_METRICS_PORT=9091`):
- `cexdex_realized_pnl_usd` — cumulative arb profit at dispatch time (monotonic)
- `cexdex_unrealized_pnl_usd` — inventory MTM drift = current - initial - realized
- `cexdex_inventory_value_usd` — current SOL+USDC MTM value
- `cexdex_initial_inventory_value_usd` — captured at first CEX price tick
- `cexdex_inventory_ratio` — SOL share of portfolio (0=all USDC, 1=all SOL)
- `cexdex_sol_price_usd` — Binance bid/ask midpoint

Grafana dashboard: `monitoring/provisioning/dashboards/cexdex-pnl.json` (auto-provisioned).

**Before going live:** balance wallet to 50/50, add detector time-dedup, expand pool list, run 4+ hours across volatility windows.

Design spec: `docs/superpowers/specs/2026-04-16-cex-dex-arb-design.md` · Implementation plan: `docs/superpowers/plans/2026-04-16-cex-dex-arb.md`

### Phase 4: MEV-Share Backruns (EVM — separate binary)
Flashbots MEV-Share on Ethereum. See `docs/STRATEGY-MEVSHARE-ETH.md`.

### All phases are Halal-compliant: spot arb only, no user fees, no borrowing, no liquidation.

## Docs

| File | Content |
|------|---------|
| `docs/DEX-REFERENCE.md` | **Primary reference.** All 9 DEX account layouts, byte offsets, quoting math, Geyser strategy |
| `docs/STRATEGY-LST-ARB.md` | LST rate arb strategy (jitoSOL/mSOL/bSOL) |
| `docs/STRATEGY-CEX-DEX-ARB.md` | CEX↔DEX arb strategy (Binance WS) |
| `docs/CEXDEX-RUN-2026-04-16.md` | First 1h dry-run analysis and recommended parameter tuning |
| `docs/STRATEGY-MEVSHARE-ETH.md` | MEV-Share on Ethereum (Flashbots) |
| `docs/superpowers/specs/` | Design specs for each feature |
| `docs/superpowers/plans/` | Implementation plans (task-by-task) |
| `docs/superpowers/specs/verified-dex-offsets.md` | Verified offsets + quoting math from production system |

## Known Pitfalls — Read Before Touching

1. **Jito mempool is DEAD.** `subscribe_mempool` was killed March 2024. Don't revive it.
2. **`jito-sdk-rust` is unnecessary.** We do raw JSON-RPC via reqwest.
3. **`solana-sdk` 4.x dropped re-exports.** `system_instruction`, `system_program`, `address_lookup_table` are now in separate crates (`solana-system-interface`, `solana-message`, `solana-address-lookup-table-interface`). `solana-system-interface` needs `features = ["bincode"]` for instruction builders.
4. **LaserStream proto types** (`helius_laserstream::grpc::*`) are from `laserstream-core-proto`, a fork of yellowstone-grpc-proto. Same structure, different crate.
5. **Base64 v0.22 API:** Uses `Engine` trait — `general_purpose::STANDARD.encode()`.
6. **DashMap `get_mut` returns `RefMut`** — must call `.value_mut()`.
7. **`crossbeam_channel::Sender::try_send`** is non-blocking — correct for stale events.
8. **LaserStream handles TLS internally** — no manual `ClientTlsConfig` needed. Connection, reconnection, and Zstd compression are handled by the SDK.
9. **Raydium CLMM tick_current is at offset 269** (not 261). sqrt_price_x64 (u128, 16B) at 253 ends at 269, tick follows.
10. **Meteora DLMM account size is 904 bytes** (not 902 or 920). Verified on mainnet.
11. **Raydium CP discriminator:** `[247, 237, 227, 245, 215, 195, 222, 70]`.
12. **Meteora DAMM v2 discriminator:** `[241, 154, 109, 4, 17, 177, 109, 188]`.
13. **RwLock in BlockhashCache is poison-tolerant** — uses `match` + `into_inner()`, not `unwrap()`.
14. **CLMM fee denominator is 1,000,000, NOT 10,000.** A 0.3% pool has feeRate=3000. Convert from fee_bps: `fee_rate = fee_bps * 100`.
15. **Never use f64 for CLMM math.** The `P * P_new` product overflows f64 precision. Use u128 with careful division ordering to avoid overflow.
16. **DLMM bin prices are precomputed on-chain.** Don't compute `(1+binStep/10000)^binId` — it overflows for real bin IDs. Parse `bin.price` (u128) from bin array accounts instead.
17. **DLMM active_id max is ~443636** (not 2^23). Values like 8388608 are garbage — skip those pools.
18. **Profit sanity cap: 1 SOL.** Any route showing >1 SOL profit is almost certainly a stale-state artifact. The simulator rejects these automatically.
19. **Phoenix/Manifest SDK crates (phoenix-v1, manifest-dex) conflict with solana-sdk 2.2.** We use raw byte-offset parsing with bytemuck instead. Do not add these crates to Cargo.toml.
20. **Phoenix market accounts are variable-size.** Can't route by data.len() like AMMs. The `try_parse_orderbook()` fallback handles this.
21. **Phoenix orderbook top-of-book requires Red-Black tree traversal.** Currently deferred — pools are discovered with zero reserves/pricing. Full book parsing needs the sokoban crate or manual tree walk.
22. **PumpSwap fees are tiered 30-125 bps by market cap**, not flat. We use 125 bps (worst-case) for conservative quoting. The on-chain Fee Program handles the actual tier.
23. **PumpSwap pool_v2 is NOT part of the PumpSwap IDL** — it's from the Pump bonding curve program. Do not include it in swap instructions.
24. **execute_arb_v2 rewrites amount_in per hop** via `amount_in_offset` in HopV2Params. Offset is 1 for Raydium AMM V4, 8 for all Anchor DEXes. The on-chain program patches ix_data with actual received amount (balance diff) before invoking the next hop.
25. **Always use multiple ALTs in V0 messages.** Our 56-addr ALT + competitor's 170-addr ALT = 226 unique addresses for maximum compression.
26. **Jito tip stream WS sends SOL floats, not lamports.** Values like `2.6665e-6` are SOL. `parse_tip_value()` auto-converts values < 1000 to lamports (multiply by 1e9). The REST API returns the same format.
27. **Route calculator uses `get_any()` (no TTL).** The simulator also uses `get_any()`. On-chain arb-guard's `min_amount_out` is the real safety gate, not cache TTL. See "Tunable Constraints" section to revert if needed.
28. **`getBundleStatuses` is heavily rate-limited** on shared Helius RPC (1 req/sec, 120s backoff). Confirmation tracker gives up after 2 RPC errors. Consider a dedicated Jito RPC endpoint for status checks.

## Tunable Constraints — Latency vs Coverage Trade-offs

These were introduced to cut pipeline latency from 15ms to <1ms. If we're missing profitable routes, these are the knobs to turn (at the cost of increased latency):

| Constraint | Location | Current | What it does | Relaxing it |
|-----------|----------|---------|-------------|-------------|
| `MAX_POOLS_PER_TOKEN` | `router/calculator.rs` | 20 | Caps pools iterated per token in route search | Increase to 50+ to find more pairs, but route calc time grows quadratically |
| `EARLY_EXIT_ROUTES` | `router/calculator.rs` | 5 | Stop searching after N profitable routes found | Increase to find more candidates, but diminishing returns |
| 3-hop gating | `calculator.rs` | Only runs if 2-hop found < EARLY_EXIT routes | 3-hop search is O(N^3) | Remove the gate to always search 3-hop (adds ~5-10ms) |
| `get_any()` in calculator | `calculator.rs` | Skips TTL check | Finds routes from stale pool data | Revert to `get()` if too many stale-state rejections |
| `get_any()` in simulator | `simulator.rs` | Skips TTL check | On-chain arb-guard is the safety net | Revert to `get()` if seeing too many on-chain failures |
| Single SOL-base search | `main.rs` | One `find_routes_for_base(SOL)` call | Was two `find_routes()` calls (trigger + reverse) | The old approach searched from the trigger token too, which found some non-SOL-base routes |
| `pool_state_ttl` | `config.rs` | 2s | Cache freshness window | Increase back to 5s if too many cache misses; decrease to 1s if state is reliably fresh |
| `SLIPPAGE_TOLERANCE` | `.env` | 0.25 | Discounts profit by 25% before tipping | Lower to 0.10 to tip more aggressively, higher to be more conservative |

**How to diagnose:** Run engine for 5 min, check `OPPORTUNITY` count vs old runs. If opportunities dropped significantly, relax `MAX_POOLS_PER_TOKEN` first (cheapest knob). If route calc time is fine but no bundles land, the issue is tip competitiveness, not route discovery.

## Environment Variables

See `.env.example`. Key ones:
- `GEYSER_GRPC_URL` — Helius LaserStream gRPC endpoint (NOT shared RPC URL)
- `GEYSER_AUTH_TOKEN` — Helius API key
- `RPC_URL` — Helius shared RPC (for blockhash, vault balance fetch)
- `JITO_BLOCK_ENGINE_URL` / `JITO_RELAY_URL` — Jito block engine
- `SEARCHER_KEYPAIR` — Path to signer keypair JSON
- `DRY_RUN=true` — Log opportunities without submitting (default)
- `MIN_PROFIT_LAMPORTS` — Minimum net profit to submit (default 100000 = 0.0001 SOL)
- `TIP_FRACTION` — Main engine: fraction of slippage-adjusted profit given as tip (default 0.50)
- `CEXDEX_TIP_FRACTION` — CEX-DEX binary: separate tip fraction (default 0.50, recommend 0.30 for thin edges)
- `SLIPPAGE_TOLERANCE` — Discount on estimated profit before tipping (default 0.25 = 25%)
- `LST_ARB_ENABLED` — Enable LST rate arb (default true)
- `LST_MIN_SPREAD_BPS` — Minimum spread for LST arb (default 5)
- `METRICS_PORT` — Prometheus `/metrics` HTTP endpoint port (disabled if unset)
- `OTLP_ENDPOINT` — OTLP HTTP endpoint for tracing span export (disabled if unset)
- `OTLP_SERVICE_NAME` — Service name in OTLP traces (default `mev-engine`)
