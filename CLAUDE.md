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

- Raydium AMM v4: `675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8`
- Raydium CLMM: `CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK`
- Orca Whirlpool: `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc`
- Meteora DLMM: `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo`

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

## What's TODO

- Pool state bootstrapping on startup (RPC getProgramAccounts → populate cache + vault index)
- CLMM tick-crossing math (current simulator uses constant-product approximation)
- Real DEX swap instruction account lists (currently placeholder single-account)
- Recent blockhash caching (~2s TTL via RPC)
- Reconnect logic for Geyser stream disconnects
- Metrics/Prometheus endpoint for monitoring
- JIT liquidity provision (Phase 2)
- Intent solving (Phase 3)

## Environment Variables

See `.env.example`. Key ones:
- `GEYSER_GRPC_URL` — Yellowstone gRPC endpoint (e.g., from Triton, Helius)
- `GEYSER_AUTH_TOKEN` — Auth token for Geyser provider
- `JITO_BLOCK_ENGINE_URL` / `JITO_RELAY_URL` — Jito block engine
- `SEARCHER_KEYPAIR` — Path to signer keypair JSON
- `DRY_RUN=true` — Log opportunities without submitting (default)
- `MIN_PROFIT_LAMPORTS` — Minimum net profit to submit (default 100000 = 0.0001 SOL)
- `TIP_FRACTION` — Fraction of profit given as Jito tip (default 0.50)
