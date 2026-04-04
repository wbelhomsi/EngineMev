# EngineMev — Solana MEV Backrun Arbitrage Engine

## Git Co-Author

When committing, always use this co-author line (not the claude-flow default):
```
Co-Authored-By: Claude <noreply@anthropic.com>
```

## What This Is

Halal-compliant MEV backrun engine on Solana. Detects price dislocations across 8 DEXes (6 AMMs + 2 CLOBs) via Yellowstone Geyser (Helius LaserStream) account streaming, then submits atomic arbitrage bundles via multi-relay fan-out (Jito, Nozomi, bloXroute, Astralane, ZeroSlot).

**Repo:** github.com/wbelhomsi/EngineMev
**Status:** DRY_RUN mode working. Detecting real arb opportunities on mainnet (~27 in 3 min, ~0.0117 SOL potential profit). Not yet submitting bundles.

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

Subscribe by **DEX program owner** — NOT by individual vault accounts or Token Program.
- Geyser streams pool state account updates when swaps happen
- Per-DEX parsers extract reserves/pricing from pool-specific layouts
- Category A (Orca, CLMM, DLMM, DAMM v2): reserves derived from pool state directly
- Category B (Raydium AMM v4, CP): lazy vault balance fetch via `getMultipleAccounts` per swap event
- Zero-bootstrap: all pools discovered lazily via Geyser (no getProgramAccounts at startup)

## Key Technical Decisions

- **No jito-sdk-rust dependency**: Raw JSON-RPC via reqwest is leaner.
- **No Jito gRPC SearcherServiceClient**: Deprecated since March 2024.
- **Yellowstone gRPC Geyser** (v12.x): Streams pool state changes from validator memory at sub-50ms via Helius LaserStream.
- **TLS required for LaserStream**: `ClientTlsConfig::new().with_native_roots()` on the gRPC builder.
- **crossbeam-channel** between async Geyser stream and sync router thread.
- **DashMap** for lock-free concurrent cache reads across threads.
- **Per-DEX parsers in stream.rs**: Route by data size (653=Orca, 1560=CLMM, 904=DLMM, 1112=DAMM v2, 752=Raydium AMM, 637=Raydium CP). Phoenix and Manifest use variable-size accounts routed by `try_parse_orderbook()` fallback instead of data size.
- **BlockhashCache**: `Arc<RwLock>` with 5s staleness, background 2s refresh via `getLatestBlockhash`.
- **API key redaction**: `config::redact_url()` strips keys from all log output.

## Module Map

```
src/
├── main.rs              # Pipeline orchestration: Geyser → Router → Bundle → Relay
│                        # Geyser reconnect with exponential backoff (1s → 30s max)
│                        # Sanctum virtual pool bootstrap, blockhash task spawn
├── lib.rs               # Re-exports modules for integration tests
├── config.rs            # Env config, 9 DEX program IDs, relay endpoints, redact_url()
├── mempool/
│   ├── mod.rs           # Exports GeyserStream, PoolStateChange
│   └── stream.rs        # Yellowstone gRPC subscription, per-DEX pool state parsers,
│                        # lazy vault fetch for Raydium, approx_reserves_from_sqrt_price
├── router/
│   ├── mod.rs           # Exports RouteCalculator, ProfitSimulator
│   ├── pool.rs          # DexType (9 variants: 6 AMMs + Sanctum + PhoenixV1 + Manifest), PoolState, ArbRoute, RouteHop, DetectedSwap
│   ├── calculator.rs    # 2-hop and 3-hop circular route discovery, O(1) via token→pool index
│   └── simulator.rs     # Final go/no-go gate: re-reads fresh state, calculates tip, checks min profit
├── executor/
│   ├── mod.rs           # Exports BundleBuilder, MultiRelay
│   ├── bundle.rs        # Builds arb tx + Jito tip, Sanctum SwapExactIn IX, minimum_amount_out enforcement
│   └── relay.rs         # Multi-relay fan-out: Jito/Nozomi/bloXroute/Astralane/ZeroSlot JSON-RPC
└── state/
    ├── mod.rs           # Exports StateCache, BlockhashCache
    ├── cache.rs         # DashMap pool cache with TTL, token→pool index, 10-min eviction
    ├── blockhash.rs     # BlockhashCache: Arc<RwLock>, 5s staleness, background 2s fetch loop
    └── bootstrap.rs     # DEPRECATED — replaced by lazy Geyser discovery
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
cargo test --test unit                        # 87 unit tests
cargo test --features e2e --test e2e          # 4 e2e tests
```

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
- Profit sanity cap (10 SOL max) catches approximation artifacts
- Route calculator (2-hop and 3-hop)
- Profit simulator with fresh-state validation and fresh hop output writeback
- Bundle builder with minimum_amount_out enforcement and correct per-hop amount_in chaining
- Total tip accounting (Jito + Astralane) — simulator rejects if total tips >= profit
- Real swap IX builders for all 9 DEXes (Raydium AMM/CP/CLMM, Orca, DLMM, DAMM v2, Sanctum, Phoenix, Manifest)
- All 8 DEXes + Sanctum enabled in can_submit_route()
- Multi-relay fan-out (Jito/Nozomi/bloXroute/Astralane/ZeroSlot) with per-relay rate limiting
- Blockhash cache (2s refresh, 5s staleness)
- Geyser reconnect with exponential backoff
- Helius LaserStream TLS connection
- API key redaction in all logs
- LST rate arb (Sanctum virtual pools, enabled for submission)
- Phoenix V1 + Manifest CLOB market parsing + swap IX builders (enabled for submission)
- LazyLock static Pubkeys — no more Pubkey::from_str() on hot path
- Pre-computed pair index in StateCache for O(1) pool pair lookups
- Token-2022 ATA derivation correct for Raydium CP and Meteora DLMM
- 87 unit tests + 4 e2e tests passing
- Tested on mainnet: ~300 realistic opportunities in 5 min, ~0.000189 SOL avg profit per opp

**Remaining:**
- CLMM multi-tick crossing (current: single-tick only, underestimates large swaps — conservative)
- DLMM bin-by-bin simulation (current: synthetic reserves from active_id — needs bin array accounts for accuracy)
- Raydium AMM v4 not yet in can_submit_route() — needs on-chain verification first
- Deduplication of repeated opportunities on same pool pair
- Metrics/Prometheus endpoint

### Phase 3: CEX↔DEX Arb (SVM — new module)
Binance websocket price feed + divergence detector. See `docs/STRATEGY-CEX-DEX-ARB.md`.

### Phase 4: MEV-Share Backruns (EVM — separate binary)
Flashbots MEV-Share on Ethereum. See `docs/STRATEGY-MEVSHARE-ETH.md`.

### All phases are Halal-compliant: spot arb only, no user fees, no borrowing, no liquidation.

## Docs

| File | Content |
|------|---------|
| `docs/DEX-REFERENCE.md` | **Primary reference.** All 9 DEX account layouts, byte offsets, quoting math, Geyser strategy |
| `docs/STRATEGY-LST-ARB.md` | LST rate arb strategy (jitoSOL/mSOL/bSOL) |
| `docs/STRATEGY-CEX-DEX-ARB.md` | CEX↔DEX arb strategy (Binance WS) |
| `docs/STRATEGY-MEVSHARE-ETH.md` | MEV-Share on Ethereum (Flashbots) |
| `docs/superpowers/specs/` | Design specs for each feature |
| `docs/superpowers/plans/` | Implementation plans (task-by-task) |
| `docs/superpowers/specs/verified-dex-offsets.md` | Verified offsets + quoting math from production system |

## Known Pitfalls — Read Before Touching

1. **Jito mempool is DEAD.** `subscribe_mempool` was killed March 2024. Don't revive it.
2. **`jito-sdk-rust` is unnecessary.** We do raw JSON-RPC via reqwest.
3. **`solana-sdk` 2.x has breaking changes from 1.x.** Verify imports if upgrading.
4. **yellowstone-grpc-proto generated types** are sensitive to proto version.
5. **Base64 v0.22 API:** Uses `Engine` trait — `general_purpose::STANDARD.encode()`.
6. **DashMap `get_mut` returns `RefMut`** — must call `.value_mut()`.
7. **`crossbeam_channel::Sender::try_send`** is non-blocking — correct for stale events.
8. **Geyser TLS required for LaserStream** — `ClientTlsConfig::new().with_native_roots()`.
9. **Raydium CLMM tick_current is at offset 269** (not 261). sqrt_price_x64 (u128, 16B) at 253 ends at 269, tick follows.
10. **Meteora DLMM account size is 904 bytes** (not 902 or 920). Verified on mainnet.
11. **Raydium CP discriminator:** `[247, 237, 227, 245, 215, 195, 222, 70]`.
12. **Meteora DAMM v2 discriminator:** `[241, 154, 109, 4, 17, 177, 109, 188]`.
13. **RwLock in BlockhashCache is poison-tolerant** — uses `match` + `into_inner()`, not `unwrap()`.
14. **CLMM fee denominator is 1,000,000, NOT 10,000.** A 0.3% pool has feeRate=3000. Convert from fee_bps: `fee_rate = fee_bps * 100`.
15. **Never use f64 for CLMM math.** The `P * P_new` product overflows f64 precision. Use u128 with careful division ordering to avoid overflow.
16. **DLMM bin prices are precomputed on-chain.** Don't compute `(1+binStep/10000)^binId` — it overflows for real bin IDs. Parse `bin.price` (u128) from bin array accounts instead.
17. **DLMM active_id max is ~443636** (not 2^23). Values like 8388608 are garbage — skip those pools.
18. **Profit sanity cap: 10 SOL.** Any route showing >10 SOL profit is an approximation artifact. The simulator rejects these automatically.
19. **Phoenix/Manifest SDK crates (phoenix-v1, manifest-dex) conflict with solana-sdk 2.2.** We use raw byte-offset parsing with bytemuck instead. Do not add these crates to Cargo.toml.
20. **Phoenix market accounts are variable-size.** Can't route by data.len() like AMMs. The `try_parse_orderbook()` fallback handles this.
21. **Phoenix orderbook top-of-book requires Red-Black tree traversal.** Currently deferred — pools are discovered with zero reserves/pricing. Full book parsing needs the sokoban crate or manual tree walk.

## Environment Variables

See `.env.example`. Key ones:
- `GEYSER_GRPC_URL` — Helius LaserStream gRPC endpoint (NOT shared RPC URL)
- `GEYSER_AUTH_TOKEN` — Helius API key
- `RPC_URL` — Helius shared RPC (for blockhash, vault balance fetch)
- `JITO_BLOCK_ENGINE_URL` / `JITO_RELAY_URL` — Jito block engine
- `SEARCHER_KEYPAIR` — Path to signer keypair JSON
- `DRY_RUN=true` — Log opportunities without submitting (default)
- `MIN_PROFIT_LAMPORTS` — Minimum net profit to submit (default 100000 = 0.0001 SOL)
- `TIP_FRACTION` — Fraction of profit given as Jito tip (default 0.50)
- `LST_ARB_ENABLED` — Enable LST rate arb (default true)
- `LST_MIN_SPREAD_BPS` — Minimum spread for LST arb (default 5)
