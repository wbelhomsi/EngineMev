# EngineMev

Halal-compliant MEV backrun arbitrage engine on Solana.

Detects price dislocations across DEXes via Yellowstone Geyser account streaming and submits atomic arbitrage bundles via multi-relay fan-out.

## How It Works

```
Geyser vault change → update pool reserves → find arb route → simulate profit → bundle + tip → relay fan-out
```

The engine observes on-chain state changes (not pending transactions — Jito mempool was killed March 2024). When a large swap moves a pool's price, the engine finds circular routes across other DEXes and submits an arbitrage bundle for the next slot.

## Supported DEXes

- Raydium AMM v4 (incl. Swap V2)
- Raydium CLMM
- Orca Whirlpool
- Meteora DLMM

## Supported Relays

Bundles are submitted to all configured relays simultaneously. Atomic execution means only one lands.

- Jito (primary)
- Nozomi
- bloXroute
- Astralane
- ZeroSlot

## Setup

```bash
# Clone
git clone git@github.com:wbelhomsi/EngineMev.git
cd EngineMev

# Configure
cp .env.example .env
# Edit .env with your Geyser endpoint, keypair paths, relay URLs

# Build
cargo build --release

# Run (dry run by default)
cargo run --release
```

## Halal Compliance

All strategies are filtered through Islamic finance principles:

- **Allowed:** Spot arbitrage, JIT liquidity provision, intent solving
- **Forbidden:** Riba (interest), liquidation bots, sandwich attacks, token sniping, any lending/borrowing protocol interaction

## Architecture

```
src/
├── main.rs           # Pipeline orchestration
├── config.rs         # Environment config, DEX program IDs
├── mempool/          # Yellowstone Geyser streaming
├── router/           # Route discovery + profit simulation
├── executor/         # Bundle building + multi-relay submission
└── state/            # DashMap pool cache with vault index
```

See `CLAUDE.md` for detailed technical documentation.

## Status

Active development. Core pipeline is scaffolded. See `CLAUDE.md` → "What's TODO" for remaining work.

## License

Private. All rights reserved.
