# CEX-DEX Arbitrage (Model A, SOL/USDC) — Design

**Date:** 2026-04-16
**Status:** Approved
**Related:** `docs/STRATEGY-CEX-DEX-ARB.md` (high-level strategy)

## Goal

Detect divergence between Binance SOL/USDC and on-chain Solana DEX pools. When the spread exceeds a configurable threshold and fees, execute a single-leg on-chain swap on the favorable side. Run as a separate binary with a dedicated wallet for clean P&L isolation.

## Non-Goals (v1)

- CEX execution leg (Model B). Inventory naturally oscillates; manual top-up when caps hit.
- Multiple token pairs. SOL/USDC only. Adding pairs later is config + symbol mapping.
- Multiple CEXes. Binance only. Binance has deepest book and lowest WS latency from Frankfurt.
- Cross-arbitrage with the existing DEX↔DEX engine. Separate binary, separate wallet.

## Why a Separate Binary

The main engine does 2-3 hop circular arbs. CEX-DEX is single-leg, non-circular, and needs CEX-priced profit math. Mixing would require changing `ArbRoute`, `ProfitSimulator`, and main.rs — all risky on a live engine. A new binary reuses the low-level crates (`router::dex` quoting, `executor::BundleBuilder`, `executor::relays`) but has its own orchestration, detector, and simulator.

```
src/bin/cexdex.rs        ← new binary entry point
src/feed/                ← new module: Binance WS feed
src/cexdex/              ← new module: detector, inventory, simulator
```

Bin/mod separation:
- `src/feed/mod.rs`, `src/feed/binance.rs` — CEX WS client
- `src/cexdex/mod.rs`, `src/cexdex/detector.rs`, `src/cexdex/inventory.rs`, `src/cexdex/simulator.rs`, `src/cexdex/route.rs`

## Architecture

```
  Binance bookTicker WS                  Helius LaserStream
  (solusdt@bookTicker)                  (SOL/USDC pool accounts)
         │                                        │
         ▼                                        ▼
  ┌───────────────┐                      ┌────────────────┐
  │  BinanceFeed  │                      │  Narrow Geyser │
  │  (WS client,  │                      │  (specific     │
  │  auto-recon.) │                      │  pool pubkeys) │
  └───────┬───────┘                      └────────┬───────┘
          │                                       │
          ▼                                       ▼
  ┌─────────────────────────────────────────────────────┐
  │              PriceStore (shared state)              │
  │  - cex_bid, cex_ask, cex_received_at (local clock)  │
  │  - per-pool: reserves, tick/bin data, last_slot     │
  └──────────────────────┬──────────────────────────────┘
                         │ (either side triggers)
                         ▼
  ┌─────────────────────────────────────────────────────┐
  │              Divergence Detector                     │
  │  1. for each monitored pool:                         │
  │     a. compute optimal trade size via quoter walk   │
  │     b. check inventory gates                         │
  │     c. compute CEX-priced profit                     │
  │  2. return best opportunity across pools             │
  └──────────────────────┬──────────────────────────────┘
                         │
                         ▼
  ┌─────────────────────────────────────────────────────┐
  │              CexDexRoute → CexDexSimulator           │
  │              → BundleBuilder → MultiRelay            │
  │              (existing crates, unchanged)            │
  └─────────────────────────────────────────────────────┘
                         │
                         ▼
                  Inventory update
             (reserve on submit,
              commit on CONFIRMED,
              release on DROPPED)
```

## Component Details

### 1. BinanceFeed (`src/feed/binance.rs`)

Connects to `wss://stream.binance.com:9443/ws/solusdc@bookTicker`. Auto-reconnect with exponential backoff (same pattern as our Jito tip floor WS).

**On each message:**
```rust
{ "u": 400900217, "s": "SOLUSDC", "b": "185.20", "B": "100.00", "a": "185.21", "A": "50.00" }
```

Parse `b` (best bid) and `a` (best ask). Write to `PriceStore` with `cex_received_at = Instant::now()` (local time — exchange timestamps are ignored to eliminate clock skew).

**Pitfall:** Binance WS drops silently after 24 hours — the server sends a close frame. We must reconnect. Our existing Jito tip floor WS handles this; reuse the same pattern.

### 2. Narrow Geyser Subscription

Unlike the main engine which subscribes by DEX program owner, this binary subscribes to a **specific list of SOL/USDC pool account pubkeys**. Tiny bandwidth. Config:

```env
CEXDEX_POOLS=RAYDIUM_CP:8sLbNZoA1cfnvMJLPfp98ZLAnFSYCFApfJKMbiXNLwxj,ORCA:...
```

Pool addresses hardcoded at startup. One subscription filter covering all monitored pools.

### 3. PriceStore

```rust
pub struct PriceStore {
    cex_bid_usd: AtomicF64,
    cex_ask_usd: AtomicF64,
    cex_received_at: Arc<RwLock<Instant>>,
    pool_cache: StateCache,  // reuses existing type
}
```

Shared between BinanceFeed, Geyser stream, and Detector. Both sides (CEX + on-chain) write independently. Reads are lock-free (atomics) except the timestamp RwLock which is cheap.

### 4. Divergence Detector (`src/cexdex/detector.rs`)

Triggered by either a CEX update OR a pool update (via crossbeam channel `DetectorEvent`). On trigger:

```
1. Check staleness:
   - CEX: (now - cex_received_at) < CEX_STALENESS_MS (default 500ms)
   - Pool: slot age < POOL_STALENESS_SLOTS (default 3 slots = 1.2s)
   Skip if either stale.

2. For each monitored pool:
   - Determine direction:
       buy_on_dex  if dex_price_to_buy_sol < cex_bid (DEX cheaper; buy SOL there, notional sell on CEX)
       sell_on_dex if dex_price_to_sell_sol > cex_ask (DEX expensive; sell SOL there)
   - Skip if neither direction has edge > MIN_SPREAD_BPS (default 15 bps).

3. Size the trade:
   - trade_size_sol = optimal_trade_size(pool, direction, inventory, config)
   - Walk trade size up: at each step, re-quote with exact `get_output_amount_with_cache()`.
     Stop when slippage erodes edge below MIN_SPREAD_BPS, or hit MAX_TRADE_SIZE_SOL, or hit inventory cap.

4. Inventory gate:
   - Check Inventory::allows(direction, size) → returns false if hard cap breached.

5. Build CexDexRoute with single hop, attach CEX-priced expected profit.
```

Takes the **best** opportunity (highest USD profit) across all pools in this tick.

### 5. CexDexRoute + CexDexSimulator

New types — NOT a reuse of `ArbRoute`/`ProfitSimulator`:

```rust
pub struct CexDexRoute {
    pub pool_address: Pubkey,
    pub dex_type: DexType,
    pub direction: ArbDirection,        // BuyOnDex | SellOnDex
    pub input_mint: Pubkey,             // USDC or SOL
    pub output_mint: Pubkey,            // the other
    pub input_amount: u64,              // atoms of input_mint
    pub expected_output: u64,           // atoms of output_mint
    pub cex_bid_at_detection: f64,
    pub cex_ask_at_detection: f64,
    pub expected_profit_usd: f64,       // CEX-priced profit (gross, before tip)
    pub observed_slot: u64,
}

impl CexDexSimulator {
    pub fn simulate(&self, route: &CexDexRoute) -> SimulationResult {
        // 1. Re-read fresh pool state (get_any, no TTL gate)
        // 2. Re-quote output at exact input_amount
        // 3. Re-price via current CEX mid
        // 4. Profit (USD) = output_usd - input_usd - tip_usd - tx_fee_usd
        // 5. Reject if profit < MIN_PROFIT_USD or < tip floor
        // 6. Return route with min_output set to input_amount equivalent at
        //    break-even CEX price (slightly loose for slippage tolerance)
    }
}
```

**min_final_output calculation** — different from the main engine:
Break-even on-chain means: `output_atoms * cex_price >= input_atoms * cex_price_other_side`. Translate into output atoms: `min_output = (input_atoms * cex_input_price) / cex_output_price * (1 - slippage_tolerance)`. Arb-guard enforces this on-chain; slippage_tolerance (default 25%) gives headroom against fast CEX moves.

### 6. Inventory (`src/cexdex/inventory.rs`)

```rust
pub struct Inventory {
    sol_on_chain_lamports: AtomicU64,
    usdc_on_chain_atoms: AtomicU64,
    sol_reserved_lamports: AtomicU64,    // pending bundles
    usdc_reserved_atoms: AtomicU64,      // pending bundles
    sol_price_usd: AtomicF64,            // from CEX mid, for ratio calc
}
```

**Reservation lifecycle:**
- `reserve(direction, amount)` on bundle submit
- `commit(direction, amount)` on CONFIRMED (deducts from on_chain, releases reservation)
- `release(direction, amount)` on DROPPED (just releases reservation)

Hooks into existing `spawn_confirmation_tracker` — add a callback parameter.

**Gates** (`allows()`):
- Compute `sol_value_usd`, `usdc_value_usd`, `ratio = sol_value / (sol_value + usdc_value)`
- Buy SOL (USDC → SOL): allowed if `ratio < 0.80` (hard cap). Preferred if `ratio < 0.40` (skewed USDC-heavy).
- Sell SOL (SOL → USDC): allowed if `ratio > 0.20` (hard cap). Preferred if `ratio > 0.60` (skewed SOL-heavy).
- In the preferred zone: normal profit threshold applies.
- Outside preferred but inside hard cap: require 2× profit threshold (we're fighting drift).
- Outside hard cap: reject and emit `INVENTORY_ALERT` log for manual top-up.

**On-chain balance refresh:** every 30 seconds, query RPC for current SOL + USDC balances. Keeps `on_chain_*` atomics honest in case of external transfers.

### 7. Binary Entry Point (`src/bin/cexdex.rs`)

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let config = CexDexConfig::from_env()?;       // CEXDEX_* env vars
    let keypair = load_keypair(&config.searcher_keypair_path)?;  // separate wallet
    let price_store = PriceStore::new();
    let inventory = Inventory::new(&config, &keypair, &http_client).await?;  // initial balance fetch
    let cex_feed = BinanceFeed::start(&price_store).await?;
    let geyser = narrow_geyser::start(&config.pools, &price_store).await?;
    let detector = Detector::new(price_store, inventory, config);
    let bundle_builder = BundleBuilder::new(...);  // existing
    let relay_dispatcher = RelayDispatcher::new(...);  // existing

    detector.run_loop(bundle_builder, relay_dispatcher).await
}
```

## Configuration

```env
# Separate wallet for P&L isolation
CEXDEX_SEARCHER_KEYPAIR=/path/to/cexdex-wallet.json

# Binance
CEXDEX_BINANCE_WS_URL=wss://stream.binance.com:9443/ws
CEXDEX_CEX_STALENESS_MS=500

# Pool targets (comma-separated DEX:PubKey pairs)
CEXDEX_POOLS=RaydiumCp:..,Orca:..,MeteoraDlmm:..

# Strategy
CEXDEX_MIN_SPREAD_BPS=15               # min divergence to consider
CEXDEX_MIN_PROFIT_USD=0.10             # min net profit in USD after tip + fees
CEXDEX_MAX_TRADE_SIZE_SOL=10.0

# Inventory gates
CEXDEX_HARD_CAP_RATIO=0.80             # reject trades that would push past 80/20
CEXDEX_PREFERRED_LOW=0.40              # normal zone starts
CEXDEX_PREFERRED_HIGH=0.60
CEXDEX_SKEWED_PROFIT_MULTIPLIER=2.0    # require 2× profit when outside preferred

# Slippage (separate from main engine's env var)
CEXDEX_SLIPPAGE_TOLERANCE=0.25

# Relays (reuse main engine's relay URLs via same env vars)
JITO_BLOCK_ENGINE_URL=...
ASTRALANE_RELAY_URL=...
```

Everything prefixed `CEXDEX_` to avoid collision with the main engine's config in the same `.env`.

## Units & Decimals (Landmine Prevention)

| Quantity | Type | Unit |
|----------|------|------|
| SOL balance | `u64` | lamports (10⁻⁹ SOL) |
| USDC balance | `u64` | atoms (10⁻⁶ USDC) |
| SOL price | `f64` | USD per SOL |
| Spread | `f64` | basis points (10⁻⁴) |
| Profit | `f64` | USD |

Single helper module `src/cexdex/units.rs`:
- `sol_lamports_to_sol(u64) -> f64`
- `sol_to_lamports(f64) -> u64`
- `usdc_atoms_to_usdc(u64) -> f64`
- `usdc_to_atoms(f64) -> u64`
- `sol_to_usdc_atoms(sol_amount, price) -> u64`
- `usdc_atoms_to_sol(usdc_atoms, price) -> u64`

All callers use these helpers. Exhaustively unit tested. No raw decimal math elsewhere.

## Error Handling

| Failure | Handling |
|---------|----------|
| Binance WS disconnect | Auto-reconnect with backoff (2s, 4s, 8s...). Staleness gate rejects trades while disconnected. |
| Geyser disconnect | LaserStream SDK auto-reconnects. Same staleness gate applies. |
| Inventory fetch fails at startup | Abort startup — we must know balances to trade safely. |
| Bundle DROPPED | Release reservation. No balance change. |
| Bundle CONFIRMED | Commit reservation. Deduct from on-chain. |
| Bundle confirmation rate-limited (can't determine) | Conservative: release after 60s. Risk: double-spend. Accept for v1; rare edge case. |
| CEX price moves while tx in flight | `slippage_tolerance` (25%) gives headroom. If we "overpaid", still profitable in expectation. |
| Inventory hard cap hit | Log `INVENTORY_ALERT`, skip trade. Wait for human top-up. |

## Testing Strategy

### Unit tests (`tests/unit/cexdex_*.rs`)
- `units.rs` — round-trip conversions, edge cases (0, u64::MAX, negative prices)
- `inventory.rs` — ratio calculation, reservation lifecycle, gate logic in all zones
- `detector.rs` — divergence math, direction detection, staleness gating
- `simulator.rs` — profit calculation with CEX prices, min_output computation
- `route.rs` — CexDexRoute serialization / structure

### Integration tests (`tests/unit/cexdex_integration.rs`)
- End-to-end detector → bundle builder flow with mock CEX prices and real pool states
- Inventory drift scenarios (one-sided market simulation)
- Staleness rejection scenarios

### E2E test (`tests/e2e/cexdex_pipeline.rs`, requires `--features e2e`)
- Synthetic Binance price feed + synthetic pool state → full pipeline → inspect generated bundle instructions
- Verify arb-guard min_amount_out is computed from CEX prices
- Verify Token-2022 handling works (USDC is SPL Token, but testing robustness)

No live testing of Binance WS in CI — that's smoke-tested manually before enabling `CEXDEX_DRY_RUN=false`.

## Rollout

1. Build binary, unit + e2e tests pass.
2. Run with `CEXDEX_DRY_RUN=true` for 24 hours. Log every detected divergence. Hand-audit a sample.
3. Fund wallet with small amount (5 SOL + 1000 USDC). Set `MAX_TRADE_SIZE_SOL=1`.
4. Run live for 1 hour. Check landing rate, profitability, inventory drift.
5. Scale up if metrics look healthy: increase trade size, increase wallet funding.
6. Second pair (SOL/USDT) once v1 stable.

## Open Questions (To Resolve During Implementation)

- **Which exact pool addresses for v1?** Spec says "Raydium, Orca, Meteora" — need to pick specific high-volume pools. Will be finalized in the implementation plan by querying Birdeye/DefiLlama for current top SOL/USDC pools by volume.
- **`CEXDEX_MIN_PROFIT_USD` default** — 0.10 USD is a placeholder. Real number depends on tip floor + tx fee + Frankfurt RPC latency. Calibrate in dry-run.

## Metrics

Prometheus counters to add (reuse existing `metrics::counters` infrastructure):
- `cexdex_divergence_detected_total{direction}`
- `cexdex_divergence_gated_total{reason}` (stale, inventory, size, profit)
- `cexdex_bundles_submitted_total`
- `cexdex_bundles_confirmed_total`
- `cexdex_profit_usd_total` (gauge, cumulative)
- `cexdex_inventory_ratio` (gauge, 0-1)
- `cexdex_cex_staleness_ms` (histogram)

Grafana dashboard extension to follow.
