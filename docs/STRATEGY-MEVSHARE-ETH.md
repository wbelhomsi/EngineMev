# Strategy: MEV-Share Backruns on Ethereum

## Overview

Flashbots MEV-Share streams "hints" about pending Ethereum transactions to searchers. We build backrun bundles that capture post-trade price dislocations — same concept as EngineMev on Solana, but on the $180M/month Ethereum MEV market.

The user whose tx we backrun gets 90% of the backrun profit redistributed automatically by MEV-Share. We keep our arb profit + 10% kickback. Nobody pays a fee — the user actually earns money from us existing.

Halal: pure spot arbitrage triggered by public transaction hints. No frontrunning (backrun only), no sandwich, no borrowing.

## Why Ethereum

- $180M/month extractable value vs Solana's $45M
- MEV-Share is permissionless — any searcher can connect
- 80%+ of Ethereum transactions already go through protected RPCs
- Flashbots Protect auto-redistributes backrun profit to users
- Mature builder market (multiple builders compete for blocks)

## Architecture — Separate Binary

This is a **separate Rust binary**, not part of EngineMev. Different chain, different data model, different bundle format. But shares the same principles and some shared library code (arb math, profit simulation).

```
engine-eth/                     # New workspace member or standalone repo
├── Cargo.toml
├── src/
│   ├── main.rs                 # Event loop: SSE → detect → bundle → submit
│   ├── config.rs               # RPC, signer, Flashbots relay URL
│   ├── mevshare/
│   │   ├── mod.rs
│   │   ├── stream.rs           # SSE client for MEV-Share event stream
│   │   └── types.rs            # PendingTransaction, TransactionHint
│   ├── detector/
│   │   ├── mod.rs
│   │   └── backrun.rs          # Analyze hint → find backrun opportunity
│   ├── pool/
│   │   ├── mod.rs
│   │   ├── uniswap_v2.rs       # UniV2 constant-product math
│   │   ├── uniswap_v3.rs       # UniV3 tick-based math
│   │   └── state.rs            # Pool state cache (pair reserves, tick, liquidity)
│   ├── executor/
│   │   ├── mod.rs
│   │   ├── bundle.rs           # Build mev_sendBundle payload
│   │   └── signer.rs           # EIP-712 Flashbots signing
│   └── rpc/
│       ├── mod.rs
│       └── provider.rs         # alloy provider for RPC + simulation
```

## MEV-Share Event Stream

MEV-Share uses Server-Sent Events (SSE) to stream pending transaction hints.

**Endpoint:** `https://mev-share.flashbots.net`

**Event format:**
```json
{
  "hash": "0xabc...",
  "logs": [
    {
      "address": "0x...",       // Contract that emitted log (e.g., Uniswap pool)
      "topics": ["0x..."],      // Event signature (e.g., Swap event)
      "data": "0x..."           // Event data (amounts, if shared)
    }
  ],
  "txs": null,                  // Full tx data (if builder shares it)
  "mevGasPrice": "0x...",       // Gas price hint
  "gasUsed": "0x..."            // Gas used hint
}
```

**What we get:** The `logs` field tells us which pool was touched and sometimes the swap amounts. This is enough to detect which token pair was affected and estimate price impact.

### Rust SSE Client

```rust
// Using reqwest-eventsource or sse-client crate
pub struct MevShareStream {
    url: String,
}

impl MevShareStream {
    pub async fn connect(&self) -> impl Stream<Item = PendingTransaction> {
        // SSE stream from mev-share.flashbots.net
        // Parse each event into PendingTransaction
    }
}
```

**Rust crate:** `mev-share-rs` from Paradigm (`github.com/paradigmxyz/mev-share-rs`) provides typed Rust bindings. Alternative: `mev-share-client-rs` from optimiz-r.

## Bundle Submission

### mev_sendBundle (Recommended)

```
POST https://relay.flashbots.net
Method: mev_sendBundle
```

**Key fields:**
```json
{
  "jsonrpc": "2.0",
  "method": "mev_sendBundle",
  "params": [{
    "version": "v0.1",
    "inclusion": {
      "block": "0x...",          // Target block number
      "maxBlock": "0x..."        // Max block (usually +3)
    },
    "body": [
      { "hash": "0xabc..." },   // Reference to user's pending tx (from hint)
      { "tx": "0x...", "canRevert": false }  // Our backrun tx
    ],
    "validity": {
      "refund": [{
        "bodyIdx": 0,
        "percent": 90            // 90% of profit to user (MEV-Share default)
      }]
    }
  }],
  "id": 1
}
```

**Auth:** Requests must be signed with searcher private key using `X-Flashbots-Signature` header (keccak256 of body, signed with secp256k1).

### Rust Implementation

```rust
use alloy::signers::local::PrivateKeySigner;

pub struct FlashbotsRelay {
    relay_url: String,
    signer: PrivateKeySigner,
    client: reqwest::Client,
}

impl FlashbotsRelay {
    pub async fn send_backrun_bundle(
        &self,
        target_tx_hash: H256,
        backrun_tx: Bytes,
        target_block: u64,
    ) -> Result<String> {
        // 1. Construct mev_sendBundle payload
        // 2. Sign with X-Flashbots-Signature
        // 3. POST to relay
        // 4. Return bundle hash
    }
}
```

## Backrun Detection Logic

When we receive a hint with Uniswap Swap logs:

1. **Identify the pool:** `logs[0].address` = pool contract
2. **Decode swap direction:** `topics[0]` = Swap event signature
3. **Estimate price impact:** If amounts are shared in `data`, calculate directly. If not, use historical average for that pool's typical swap size.
4. **Find arb route:** Same pool on different DEX (UniV2 ↔ UniV3 ↔ Sushi ↔ Curve), or triangle route through related pairs.
5. **Simulate:** Use `eth_call` with state overrides to simulate the backrun at the post-swap state.
6. **Build bundle:** Reference target tx hash + our backrun tx.

## DEX Coverage

| DEX           | Type              | Router/Factory                          |
|---------------|-------------------|-----------------------------------------|
| Uniswap V2    | Constant product  | `0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f` |
| Uniswap V3    | Concentrated liq  | `0x1F98431c8aD98523631AE4a59f267346ea31F984` |
| SushiSwap     | Constant product  | `0xC0AEe478e3658e2610c5F7A4A2E1777cE9e4f2Ac` |
| Curve         | StableSwap        | Various pools                           |
| Balancer V2   | Weighted/stable   | `0xBA12222222228d8Ba445958a75a0704d566BF2C8` |

## Dependencies (Cargo.toml for engine-eth)

```toml
[dependencies]
alloy = { version = "0.12", features = ["full"] }
alloy-provider = "0.12"
alloy-signer-local = "0.12"
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["json"] }
reqwest-eventsource = "0.6"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
dashmap = "6"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
anyhow = "1"

# MEV-Share client (optional — can do raw SSE)
# mev-share-rs = "0.1"
```

**Note:** Use `alloy` (not `ethers-rs`). Ethers-rs is deprecated as of 2024. Alloy is the current Rust Ethereum SDK maintained by Paradigm/Alloy team. **Verify version on crates.io before building.**

## Config

```env
# Ethereum RPC (needs archive node for state simulation)
ETH_RPC_URL=https://eth-mainnet.g.alchemy.com/v2/YOUR_KEY
ETH_WS_URL=wss://eth-mainnet.g.alchemy.com/v2/YOUR_KEY

# MEV-Share
MEVSHARE_SSE_URL=https://mev-share.flashbots.net
FLASHBOTS_RELAY_URL=https://relay.flashbots.net

# Searcher identity
ETH_SEARCHER_PRIVATE_KEY=0x...

# Strategy
ETH_MIN_PROFIT_WEI=10000000000000000   # 0.01 ETH minimum
ETH_DRY_RUN=true
```

## Risk Controls

1. **canRevert: false** on our backrun tx — if our arb reverts, entire bundle is dropped. We never pay for failed attempts.
2. **Simulation before submission:** `eth_call` with state overrides to verify profitability.
3. **Block targeting:** Submit for current block + 1-3. Stale bundles auto-expire.
4. **No frontrunning:** MEV-Share enforces backrun-only via bundle ordering. The user's tx always executes first.
5. **Profit floor:** Hard minimum in config. Don't chase dust.

## Estimated Profitability

- Ethereum MEV market: ~$180M/month total extractable
- Backrun arb: ~30-40% of total MEV ($54-72M/month)
- Competitive searcher landscape: top 10 searchers capture ~80%
- Realistic capture for a new entrant: 0.01-0.1% = $5,400-54,000/month
- Requires: fast RPC, good simulation, competitive tip pricing

## Implementation Priority

1. SSE event stream client (connect to MEV-Share, parse hints)
2. Uniswap V2/V3 pool state cache (fetch pair reserves/ticks via RPC)
3. Backrun detector (hint → affected pool → arb route)
4. Bundle builder with Flashbots signing
5. `eth_call` simulation for profit verification
6. DRY_RUN mode with logging
7. Go live on mainnet
