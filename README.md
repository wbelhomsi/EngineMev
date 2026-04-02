# EngineMev

Halal-compliant MEV backrun arbitrage engine on Solana.

Detects price dislocations across 8 DEXes via Yellowstone Geyser (Helius LaserStream) and submits atomic arbitrage bundles via multi-relay fan-out.

## How It Works

```
Geyser pool state change → per-DEX parser → update cache → find arb route
  → simulate profit → build bundle + ATA creation + tip → relay fan-out → next slot
```

The engine observes on-chain pool state changes (not pending transactions — Jito mempool was killed March 2024). When a swap moves a pool's price, the engine finds circular routes across other DEXes and submits an arbitrage bundle for the next slot.

**Zero bootstrap.** Pools are discovered lazily via Geyser events — no `getProgramAccounts` startup delay.

## Supported DEXes

| DEX | Type | Status |
|-----|------|--------|
| Raydium AMM v4 | Constant Product | Real swap IX |
| Raydium CP (CPMM) | Constant Product | Real swap IX |
| Raydium CLMM | Concentrated Liquidity | Real swap IX |
| Orca Whirlpool | Concentrated Liquidity | Real swap IX |
| Meteora DLMM | Bin-based CLMM | Real swap IX |
| Meteora DAMM v2 | Dual-mode (CP + Concentrated) | Real swap IX |
| Phoenix V1 | Central Limit Order Book | Parser only |
| Manifest | Central Limit Order Book | Parser only |

All pool state parsing uses verified byte offsets. See `docs/DEX-REFERENCE.md` for full account layouts and quoting math.

## Supported Relays

Bundles are submitted to all configured relays simultaneously. Atomic execution means only one lands.

| Relay | Auth | Notes |
|-------|------|-------|
| Jito | Optional UUID | Primary. Frankfurt regional endpoint for low latency |
| Astralane | API key | `revert_protect=true` for zero-loss failed bundles. FRA direct IP |
| Nozomi | API key | Jito-compatible |
| bloXroute | Auth header | REST API |
| ZeroSlot | Invite | Jito-compatible |

## Setup

```bash
# Clone
git clone git@github.com:wbelhomsi/EngineMev.git
cd EngineMev

# Configure
cp .env.example .env
# Edit .env:
#   RPC_URL          — Helius shared RPC
#   GEYSER_GRPC_URL  — Helius LaserStream (NOT shared RPC)
#   GEYSER_AUTH_TOKEN — Helius API key
#   SEARCHER_PRIVATE_KEY — base58 private key (or SEARCHER_KEYPAIR for JSON file)
#   DRY_RUN=true     — set false when ready to submit real bundles

# Build
cargo build --release

# Run (dry run by default)
cargo run --release

# Tests
cargo test --test unit                   # 66 unit tests
cargo test --features e2e --test e2e     # 4 e2e tests
```

## Halal Compliance

All strategies are filtered through Islamic finance principles:

- **Allowed:** Spot arbitrage, JIT liquidity provision, intent solving
- **Forbidden:** Riba (interest), liquidation bots, sandwich attacks, token sniping, any lending/borrowing protocol interaction

## Architecture

```
src/
├── main.rs           # Pipeline orchestration, Geyser reconnect, dedup, rate limiting
├── lib.rs            # Re-exports for integration tests
├── config.rs         # Environment config, 9 DEX program IDs, redact_url()
├── mempool/
│   └── stream.rs     # Yellowstone gRPC, per-DEX pool state parsers, lazy vault fetch
├── router/
│   ├── pool.rs       # DexType (9 variants), PoolState, PoolExtra, ArbRoute
│   ├── calculator.rs # 2-hop and 3-hop circular route discovery
│   └── simulator.rs  # Fresh-state validation, profit sanity cap
├── executor/
│   ├── bundle.rs     # Swap IX builders (6 DEXes), ATA creation, Sanctum IX
│   └── relay.rs      # Multi-relay fan-out with warmup and TCP keepalive
└── state/
    ├── cache.rs      # DashMap pool cache, token→pool index
    └── blockhash.rs  # Background 2s refresh, 5s staleness guard
```

## Docs

| File | Description |
|------|-------------|
| `CLAUDE.md` | Complete technical documentation — architecture, known pitfalls, roadmap |
| `docs/DEX-REFERENCE.md` | Account layouts, byte offsets, quoting math for all 8 DEXes |
| `docs/SWAP-IX-REFERENCE.md` | Swap instruction accounts, PDAs, discriminators for all 6 AMMs |

## Status

Live on mainnet in dry-run mode. Detecting real arb opportunities (~50/min). Bundles reaching Jito at ~65ms latency. Final blocker: Token-2022 mint detection for ATA creation.

## License

Private. All rights reserved.
