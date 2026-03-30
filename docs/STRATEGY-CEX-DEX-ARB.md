# Strategy: CEX↔DEX Arbitrage

## Overview

Monitor CEX spot prices via websocket. When on-chain DEX pool price diverges from CEX, execute on-chain swap to capture the spread. Profit comes purely from market inefficiency — no user fees, no frontrunning, no protocol interaction beyond spot DEXes.

Halal: pure spot arbitrage. No borrowing, no leverage, no lending protocol.

## Why This Works

CEX prices update continuously via order book. On-chain AMM prices only update when someone swaps. During volatility, AMM pools can lag CEX by 0.1%-2%+ for seconds at a time. That's the window.

Solana's 400ms slots + Jito bundles make execution fast enough to compete. The Geyser pipeline we already have provides the on-chain side.

## Architecture

```
┌─────────────────────┐     ┌──────────────────────┐
│  CEX WebSocket Feed │     │  Geyser Account Stream│
│  (Binance bookTicker│     │  (pool vault balances) │
│   best bid/ask)     │     │                        │
└────────┬────────────┘     └───────────┬────────────┘
         │                              │
         ▼                              ▼
┌─────────────────────────────────────────────────────┐
│              Price Divergence Detector               │
│                                                      │
│  CEX mid price vs AMM implied price (from reserves)  │
│  Threshold: divergence > min_spread_bps              │
│  Direction: which side is cheap?                     │
└────────────────────┬────────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────┐
│              Profit Calculator                       │
│                                                      │
│  gross = |cex_price - dex_price| * trade_size        │
│  costs = jito_tip + tx_fee + slippage_estimate       │
│  net = gross - costs                                 │
│  gate: net > MIN_PROFIT_LAMPORTS                     │
└────────────────────┬────────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────┐
│              Execution (two legs)                    │
│                                                      │
│  ON-CHAIN: Jito bundle (swap on DEX via existing     │
│            BundleBuilder + MultiRelay pipeline)      │
│                                                      │
│  CEX: REST API market order (opposite direction)     │
│       OR: pre-positioned inventory (no CEX trade)    │
└─────────────────────────────────────────────────────┘
```

## Two Execution Models

### Model A: Inventory-Based (Simpler, Recommended to Start)

Hold SOL + USDC (or other base pairs) in your on-chain wallet. When DEX is cheap, buy on-chain. When DEX is expensive, sell on-chain. Your inventory naturally oscillates. No CEX leg needed at all — you just rebalance periodically.

Advantages: single-leg execution (on-chain only), no CEX API, no counterparty risk, reuses EngineMev pipeline 100%.

Disadvantage: requires starting capital, inventory risk if price trends one direction.

### Model B: Two-Leg (Higher capital efficiency)

Execute simultaneously on-chain and on CEX. Buy cheap side, sell expensive side. Delta-neutral per trade.

Advantages: no inventory risk, delta neutral.

Disadvantage: needs CEX API integration, latency on CEX leg, capital split across two venues.

**Start with Model A.** It's the same codebase as EngineMev with a CEX price feed bolted on.

## Key Components to Build

### 1. CEX Price Feed (`src/feed/`)

```
New module: src/feed/
├── mod.rs          # Exports CexFeed, CexPrice
├── binance.rs      # Binance bookTicker websocket
└── bybit.rs        # Bybit (optional secondary feed)
```

**Binance bookTicker stream:**
- Endpoint: `wss://stream.binance.com:9443/ws/solusdt@bookTicker`
- Payload: `{ "s": "SOLUSDT", "b": "185.20", "B": "100", "a": "185.21", "A": "50" }`
- Fields: best bid price (b), best ask price (a)
- Latency: ~10-50ms to receive
- Rust crate: `tokio-tungstenite` for raw WS, or `binance-rs` for typed API

```rust
pub struct CexPrice {
    pub symbol: String,       // "SOL/USDT"
    pub best_bid: f64,
    pub best_ask: f64,
    pub mid: f64,             // (bid + ask) / 2
    pub timestamp_us: u64,    // microsecond precision
}

pub struct CexFeed {
    // Shared atomic price store — updated by WS reader, read by detector
    prices: Arc<DashMap<String, CexPrice>>,
}
```

### 2. Price Divergence Detector (`src/detector/`)

Compares CEX mid price against AMM implied price (calculated from pool reserves).

```rust
pub struct DivergenceEvent {
    pub pool_address: Pubkey,
    pub dex_type: DexType,
    pub cex_mid: f64,
    pub dex_implied: f64,
    pub spread_bps: f64,      // basis points of divergence
    pub direction: ArbDirection,
    pub slot: u64,
}

pub enum ArbDirection {
    BuyOnDex,   // DEX is cheaper than CEX
    SellOnDex,  // DEX is more expensive than CEX
}
```

**AMM implied price** from pool reserves:
```
price_a_in_b = reserve_b / reserve_a  (constant product)
```

### 3. Integration with Existing Pipeline

The beauty: once we detect a divergence, the rest is identical to EngineMev.

- `DivergenceEvent` → construct `ArbRoute` with single hop (buy/sell on the cheap pool)
- Feed into existing `ProfitSimulator` → `BundleBuilder` → `MultiRelay`
- Same Jito tip logic, same fan-out

## Token Pairs to Monitor

Start with highest-volume Solana pairs that also trade on Binance:

| Pair      | CEX Symbol  | On-chain pools                    |
|-----------|-------------|-----------------------------------|
| SOL/USDC  | SOLUSDC     | Raydium, Orca, Meteora            |
| SOL/USDT  | SOLUSDT     | Raydium, Orca                     |
| RAY/USDC  | RAYUSDC     | Raydium                           |
| JTO/USDC  | JTOUSDC     | Raydium, Orca                     |
| JUP/USDC  | JUPUSDC     | Raydium, Orca, Meteora            |
| BONK/USDC | BONKUSDC    | Raydium, Orca                     |
| WIF/USDC  | WIFUSDC     | Raydium, Orca                     |

## Config Additions

```env
# CEX Feed
BINANCE_WS_URL=wss://stream.binance.com:9443/ws
BINANCE_API_KEY=           # Only needed for Model B (CEX execution)
BINANCE_API_SECRET=        # Only needed for Model B

# CEX-DEX Strategy
MIN_SPREAD_BPS=15          # Minimum divergence to act (15 bps = 0.15%)
MAX_TRADE_SIZE_SOL=10      # Max SOL per arb trade
CEX_DEX_ENABLED=true       # Feature flag
```

## Risk Controls

1. **Stale price gate**: If CEX price is >500ms old, skip. Stale feed = wrong decision.
2. **Max position size**: Cap per-trade size to limit inventory risk.
3. **Inventory limits**: In Model A, stop buying if SOL balance > threshold, stop selling if USDC balance > threshold.
4. **Rate limit**: Max 1 arb per pool per slot (400ms). Don't spam.
5. **Kill switch**: If 3 consecutive bundles revert, pause and alert.

## Estimated Profitability

- Average SOL/USDC spread during volatility: 10-50 bps
- After tip + tx fee (~5-10 bps): 5-40 bps net
- At 10 SOL per trade, 5 bps = 0.005 SOL = ~$0.90
- Volume: 50-200 opportunities/hour during active markets
- Conservative estimate: $50-200/day with 50 SOL capital

## Implementation Priority

1. `CexFeed` with Binance bookTicker WS (SOL/USDC only)
2. `DivergenceDetector` comparing CEX mid vs AMM implied
3. Wire into existing pipeline (reuse BundleBuilder + MultiRelay)
4. Test in DRY_RUN with real data
5. Go live with 5 SOL, scale up
