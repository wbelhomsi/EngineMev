# EngineMev — Solana MEV Backrun Arbitrage Engine

## What This Is

Halal-compliant MEV backrun engine on Solana. Detects price dislocations across DEXes via Yellowstone Geyser account streaming, then submits atomic arbitrage bundles via multi-relay fan-out (Jito, Nozomi, bloXroute, Astralane, ZeroSlot).

**Repo:** github.com/wbelhomsi/EngineMev

## Halal Compliance — Non-Negotiable

All strategies MUST be Halal. This is a hard constraint, not a preference.

- **Allowed:** Spot arbitrage, JIT liquidity provision on spot DEX pools, intent solving
- **Forbidden:** Riba (interest/usury), debt exploitation (no liquidation bots), maysir (gambling/token sniping), sandwich attacks, any lending protocol interaction, leveraged positions
- Never suggest or build anything that touches lending, borrowing, or liquidation

## Architecture

Post-mempool design (Jito mempool was killed March 2024):

```
Yellowstone Geyser → vault balance change → update StateCache reserves
  → detect price dislocation → find arb route → simulate profit
  → build bundle (arb tx + Jito tip) → multi-relay fan-out → next slot
```

This is NOT same-block backrunning. We observe state changes post-block and submit for the next slot.

## Key Technical Decisions

- **No jito-sdk-rust dependency**: Raw JSON-RPC via reqwest is leaner. `POST {block_engine}/api/v1/bundles` with `sendBundle` method, base64-encoded txs.
- **No Jito gRPC SearcherServiceClient**: Deprecated. The old `subscribe_mempool` returns `Unimplemented`. Don't use it.
- **Yellowstone gRPC Geyser** (v12.2): Streams account state changes from validator memory at sub-50ms. This is the correct data source.
- **crossbeam-channel** between async Geyser stream and sync router thread: Router is pure CPU, no async overhead on hot path.
- **DashMap** for lock-free concurrent cache reads across threads.
- **Vault→Pool index** in StateCache: Geyser gives vault addresses, index maps them to pool + side (token_a or token_b).

## Module Map

```
src/
├── main.rs              # Pipeline orchestration: Geyser → Router → Bundle → Relay
├── config.rs            # Env config, DEX program IDs, relay endpoints
├── mempool/
│   ├── mod.rs           # Exports GeyserStream, PoolStateChange
│   └── stream.rs        # Yellowstone gRPC subscription, SPL Token balance parsing
├── router/
│   ├── mod.rs           # Exports RouteCalculator, ProfitSimulator
│   ├── pool.rs          # Core types: PoolState, ArbRoute, RouteHop, DetectedSwap, DexType
│   ├── calculator.rs    # 2-hop and 3-hop circular route discovery, O(1) via token→pool index
│   └── simulator.rs     # Final go/no-go gate: re-reads fresh state, calculates tip, checks min profit
├── executor/
│   ├── mod.rs           # Exports BundleBuilder, MultiRelay
│   ├── bundle.rs        # Builds arb tx + Jito tip, tip account rotation, dynamic tip floor API
│   └── relay.rs         # Multi-relay fan-out: Jito/Nozomi/bloXroute/Astralane/ZeroSlot JSON-RPC
└── state/
    ├── mod.rs           # Exports StateCache
    └── cache.rs         # DashMap pool cache with TTL, token→pool and vault→pool indices
```

## DEX Program IDs (verified current)

- Raydium AMM v4: `675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8` (752 bytes, no Anchor)
- Raydium CP (CPMM): `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C` (637 bytes, Anchor)
- Raydium CLMM: `CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK` (1560 bytes, Anchor)
- Orca Whirlpool: `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc` (653 bytes, Anchor)
- Meteora DLMM: `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo` (904 bytes, Anchor)
- Meteora DAMM v2: `cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG` (1112 bytes, Anchor)
- Sanctum S Controller: `5ocnV1qiCgaQR8Jb8xWnVbApfaygJ8tNoZfgPwsgx9kx`

See `docs/DEX-REFERENCE.md` for full account layouts and quoting math.

## Jito Tip Accounts (8 total, rotated per bundle)

Hardcoded in `bundle.rs`. Dynamic fetch available via `getTipAccounts` JSON-RPC. Minimum tip: 1000 lamports. Auctions every 200ms. Tip floor REST: `https://bundles-api-rest.jito.wtf/api/v1/bundles/tip_floor`

## Build & Run

```bash
cargo check          # Verify compilation
cargo build --release # Release build (LTO fat, codegen-units=1)
cp .env.example .env  # Configure endpoints and keys
cargo run             # Starts in DRY_RUN=true by default
```

## Critical Rules for Development

1. **ALWAYS web-search to verify any external API, SDK, or crate is current before using it.** We lost a full session building on the dead Jito mempool API. Training data goes stale.

2. **Prefer to fail than to send a losing transaction.** Every gate (simulator, minimum profit threshold) must default to rejection. No partial bets, no "maybe profitable" submissions.

3. **Every millisecond matters.** This is a latency game. Avoid unnecessary allocations on the hot path, keep the router sync (no async overhead), use pre-computed indices.

4. **Raydium Swap V2 (Sept 2025):** Reduced from 18 to 8 accounts. V1 still works. Use V2 for smaller tx size.

5. **SPL Token account layout:** Balance at bytes 64..72 (u64 LE). This is how we detect vault changes from Geyser.

## Roadmap — Multi-Strategy Plan

### Phase 1: EngineMev Core (Current — SVM)
Get the base DEX↔DEX backrun arb working end to end on Solana mainnet.

Remaining work:
- Pool state bootstrapping on startup (RPC getProgramAccounts → populate cache + vault index)
- CLMM tick-crossing math (current simulator uses constant-product approximation)
- Real DEX swap instruction account lists (currently placeholder single-account)
- Recent blockhash caching (~2s TTL via RPC)
- Reconnect logic for Geyser stream disconnects
- Metrics/Prometheus endpoint for monitoring

### Phase 2: LST Rate Arb (SVM — bolt-on, ~50 lines of new code)
Add jitoSOL, mSOL, bSOL pools to the monitored set. Same pipeline, just new token addresses. See `docs/STRATEGY-LST-ARB.md`.

### Phase 3: CEX↔DEX Arb (SVM — new module)
Binance websocket price feed + divergence detector. Inventory-based model (no CEX execution needed). Reuses existing BundleBuilder + MultiRelay. See `docs/STRATEGY-CEX-DEX-ARB.md`.

### Phase 4: MEV-Share Backruns (EVM — separate binary)
Flashbots MEV-Share on Ethereum. Separate Rust binary using `alloy`. SSE event stream → backrun detection → mev_sendBundle. See `docs/STRATEGY-MEVSHARE-ETH.md`.

### All phases are Halal-compliant: spot arb only, no user fees, no borrowing, no liquidation.

## Strategy Docs

Architecture docs live in `docs/`:
- `docs/STRATEGY-CEX-DEX-ARB.md` — CEX↔DEX arbitrage (Binance WS + on-chain, inventory model)
- `docs/STRATEGY-LST-ARB.md` — LST rate arbitrage (jitoSOL/mSOL/bSOL cross-pool)
- `docs/STRATEGY-MEVSHARE-ETH.md` — MEV-Share backruns on Ethereum (Flashbots, alloy, separate binary)

## DEX Reference

**`docs/DEX-REFERENCE.md`** — Authoritative reference for all 7 supported DEX programs. Contains verified account layouts with byte offsets, quoting math (constant product, CLMM tick-crossing, DLMM bin simulation), SPL Token vault layout, Geyser subscription strategy, and getProgramAccounts filters. Read this before touching any DEX-specific code.

## Known Pitfalls — Read Before Touching

1. **Jito mempool is DEAD.** `subscribe_mempool` was killed March 2024. Any code referencing `SearcherServiceClient`, `jito-protos`, or `jito-searcher-client` crates is dead code. Don't revive it.
2. **`jito-sdk-rust` is unnecessary.** We do raw JSON-RPC via reqwest. The SDK just wraps the same HTTP calls. Don't add it.
3. **`solana-sdk` 2.x has breaking changes from 1.x.** `Keypair` moved modules, `Transaction` signing API changed. Verify imports if upgrading.
4. **yellowstone-grpc-proto generated types** are sensitive to proto version. If upgrading `yellowstone-grpc-client`/`yellowstone-grpc-proto`, check field names in `SubscribeRequestFilterAccounts` and `SubscribeRequest` against the proto definition at `github.com/rpcpool/yellowstone-grpc`.
5. **Base64 v0.22 API:** Uses `Engine` trait — `general_purpose::STANDARD.encode()`, not the old free function `base64::encode()`.
6. **DashMap `get_mut` returns `RefMut`** — must call `.value_mut()` to get the inner reference. Don't try to deref directly.
7. **`crossbeam_channel::Sender::try_send`** is non-blocking — correct for Geyser stream (stale events are worthless). Never use blocking `send` in the stream loop.

## Compile Check Before Push

Always run `cargo check` and `cargo clippy` before committing. The project should compile with zero warnings (we cleaned all unused imports and dead code).

## Environment Variables

See `.env.example`. Key ones:
- `GEYSER_GRPC_URL` — Yellowstone gRPC endpoint (e.g., from Triton, Helius)
- `GEYSER_AUTH_TOKEN` — Auth token for Geyser provider
- `JITO_BLOCK_ENGINE_URL` / `JITO_RELAY_URL` — Jito block engine
- `SEARCHER_KEYPAIR` — Path to signer keypair JSON
- `DRY_RUN=true` — Log opportunities without submitting (default)
- `MIN_PROFIT_LAMPORTS` — Minimum net profit to submit (default 100000 = 0.0001 SOL)
- `TIP_FRACTION` — Fraction of profit given as Jito tip (default 0.50)
