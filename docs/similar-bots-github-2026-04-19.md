# Similar Open-Source Bots — GitHub Research
*Compiled 2026-04-19. Focus: retail-capital halal MEV/arb/MM on Solana and EVM.*

---

## TL;DR — Top 5 by "Code We Can Actually Learn From"

| Rank | Repo | Stars | Why It Matters |
|------|------|-------|---------------|
| 1 | [hummingbot/hummingbot](https://github.com/hummingbot/hummingbot) | 18.2k | Production cross-exchange MM with battle-tested inventory skew + one-leg fill queue — directly applicable to CEX-DEX hedging |
| 2 | [jito-labs/mev-bot](https://github.com/jito-labs/mev-bot) | 1.2k | Canonical Solana backrun arb reference; abandoned but issues reveal exactly the infra walls we already hit |
| 3 | [0xNineteen/solana-arbitrage-bot](https://github.com/0xNineteen/solana-arbitrage-bot) | 800 | Honest open-source Solana DEX arb; brute-force route search + on-chain slippage trick worth stealing |
| 4 | [nautechsystems/nautilus_trader](https://github.com/nautechsystems/nautilus_trader) | ~5k | Rust-native event-driven engine, Binance + Bybit WebSocket connectors, multi-venue framework closest to what we need for CEX-CEX |
| 5 | [hzjken/crypto-arbitrage-framework](https://github.com/hzjken/crypto-arbitrage-framework) | 698 | LP-based route optimizer with an honest writeup on the cross-exchange capital-lock problem; the `consider_inter_exc_bal` constraint is the correct mental model |

---

## 1. hummingbot/hummingbot

**URL:** https://github.com/hummingbot/hummingbot  
**Stars:** 18,200 | **Last release:** v2.13.0 (March 2026) | **Language:** Python 97%  
**Halal posture:** Mixed. Core spot MM and cross-exchange arb strategies are clean. Platform also ships perps connectors and a Deribit integration — those modules are irrelevant to us but present.

**What it actually does:** Framework for deploying market-making and arbitrage strategies across 140+ venues. The two strategies relevant to us are `cross_exchange_market_making` (maker on one venue, taker hedge on another) and `pure_market_making` with inventory skew.

**What it does well:**
- *Inventory skew* in `pure_market_making.pyx`: tracks `_filled_buys_balance` vs `_filled_sells_balance`, applies a bid/ask ratio adjustment toward a target base-asset percentage. Adjusts order sizes so that excess inventory is worked off gradually rather than in one shot.
- *One-leg fill queue* in `cross_exchange_market_making.py`: maker fills land in `_order_fill_buy_events` / `_order_fill_sell_events`. `check_and_hedge_orders()` drains the queue with asymmetric slippage buffers (`×(1 - slippage_buffer)` for sell-hedges, `×(1 + slippage_buffer)` for buy-hedges). New maker orders are *blocked* until the queue is empty — prevents doubling up exposure.
- *Anti-hysteresis timer*: prevents rapid order repricing within a configured window; avoids fee churn.
- *Retry logic on failed hedges*: if taker order fails, the fill stays in the queue and `check_and_hedge_orders()` retries next tick.

**What's missing / broken:**
- Python — latency floor is ~10ms per event loop iteration, not sub-ms.
- The `inventory_cost` feature raises a hard exception if the initial price isn't set at startup. Easy to hit in cold-start.
- No native Geyser/Yellowstone integration; Solana connector is through Jupiter REST only.

**Steal:** The fill-queue + hedge pattern and inventory skew math are the exact mechanisms we need for the CEX-DEX binary once we add a two-sided position (Model B). Port the logic to Rust, not the Python.

---

## 2. jito-labs/mev-bot

**URL:** https://github.com/jito-labs/mev-bot  
**Stars:** 1,200 | **Last maintained:** effectively dead (abandoned ~late 2023) | **Language:** TypeScript  
**Halal posture:** Spot arb only, no liquidations. Uses Solend flashloans which are interest-bearing — technically touches Riba. Not a model to copy wholesale.

**What it actually does:** Monitored the public Jito mempool for large swaps, computed backrun routes across Raydium/Raydium CLMM/Orca, executed via Jupiter with Solend flashloans.

**What it does well (historically):**
- Lookup-table caching to compress bundle transaction sizes — same problem we solved with ALTs.
- Multi-threaded worker architecture: one thread per DEX market for route calculation — matches our sync router thread design.
- "Spam decreasing amounts and let the largest land" execution strategy — addresses the slippage/amount uncertainty problem. We currently set a single `min_amount_out`; spamming descending sizes is a valid alternative.

**What's missing / broken:**
- Issue #11: "No mempool... now, what?" — the Jito public mempool was killed March 2024. The entire detection mechanism is gone.
- Issue #14 (Dec 2024): "Is this bot still working?" — No.
- Issue #6: `Value is larger than Number.MAX_SAFE_INTEGER` — JS integer overflow on lamport amounts. We avoided this entirely by using Rust i128.
- Abandoned precisely because the mempool shutdown made reactive arb impossible without Geyser account streaming (which is what we built).

**Key lesson confirmed:** Post-mempool Solana arb requires Geyser account state streaming, not mempool watching. We already made this call correctly.

---

## 3. 0xNineteen/solana-arbitrage-bot

**URL:** https://github.com/0xNineteen/solana-arbitrage-bot  
**Stars:** 800 | **Last release:** none published | **Language:** Rust  
**Halal posture:** Pure spot DEX arb. No lending, no liquidation.

**What it actually does:** Off-chain Rust bot detecting price discrepancies across Serum, Aldrin, Saber, Mercurial, Orca. Brute-force route search. Reverse-engineers DEX interactions through the Jupiter SDK. Executes via an on-chain swap program.

**What it does well:**
- *Mainnet fork unit tests*: Rust tests that fork mainnet state to verify swap quote accuracy before shipping. We have unit tests but not mainnet-fork tests — this would catch stale-offset bugs faster.
- *On-chain slippage handling*: uses a custom on-chain program that rewrites `amount_in` per hop from actual balance diffs, same as our `execute_arb_v2`. Convergent design independently validated.
- *Brute-force route search*: author argues it's faster than Bellman-Ford for the pool counts typical in Solana. Matches our approach (cap pools per token, early exit).
- Creator quote: "the life of a lone searcher is a lonely one...i realized this is not what i'm about and thus i open source." — confirms the profitability wall is real and well-known.

**What's missing / broken:**
- DEX list is 2022-era (Serum is dead). No Meteora, no Manifest, no PumpSwap.
- No Geyser streaming — polling-based, high latency.
- No Jito bundle support.

**Steal:** The mainnet-fork Rust test pattern. Add to `tests/` to validate our per-DEX parsers against real account data snapshots.

---

## 4. nautechsystems/nautilus_trader

**URL:** https://github.com/nautechsystems/nautilus_trader  
**Stars:** ~5,000 | **Last release:** active (2026) | **Language:** Rust core, Python strategy layer  
**Halal posture:** Framework is neutral. Ships perps/options connectors (Deribit, Bybit perps) but the spot connectors are clean.

**What it actually does:** Production-grade event-driven trading engine. Rust core for order routing and market data, Python for strategy logic. Deterministic backtesting with the same codepath as live trading. Binance spot connector actively maintained; Bybit WebSocket API trading added Dec 2024, TP/SL and batch order support in 2025.

**What it does well:**
- *Deterministic backtest = live parity*: same Rust event loop runs both. If we build CEX-CEX arb, testing the strategy against historical L2 orderbook data with the same execution logic is non-trivial to build from scratch — Nautilus solves this.
- *Multi-venue cross-asset order management*: built for strategies that place orders on two exchanges simultaneously.
- *Binance + Bybit WebSocket connectors*: battle-tested implementations of the exact feed we're building. Worth reading the connector code to check our `bookTicker` implementation covers all edge cases (reconnect, sequence number gaps, rate limit handling).

**What's missing / broken:**
- 64 open issues; API unstable pre-2.x.
- Python strategy layer means hot-path is not sub-ms. For a CEX-CEX arb signal that needs to place two orders in under 10ms, the Python overhead may be acceptable; for Solana Geyser latency this would not be.
- Windows only gets 64-bit precision vs 128-bit on Linux/macOS — a silent numeric difference.

**Steal:** Read the Binance and Bybit WebSocket connector implementations (`nautilus_trader/adapters/binance/` and `bybit/`) to verify our reconnect + sequence-gap handling matches theirs.

---

## 5. hzjken/crypto-arbitrage-framework

**URL:** https://github.com/hzjken/crypto-arbitrage-framework  
**Stars:** 698 | **Last commit:** single contributor, ~2019-2020 | **Language:** Python  
**Halal posture:** Spot only. Uses CCXT for execution. No lending, no perps.

**What it actually does:** CPLEX-based LP optimizer that finds the maximum-return multi-exchange arbitrage path given capital constraints, then computes optimal trade amounts per pair. Not a live bot — a research framework.

**What it does well:**
- *`consider_inter_exc_bal` constraint*: explicitly models the fact that cross-exchange withdrawal takes hours. Requires that each exchange wallet holds enough capital to execute its leg independently without waiting for a transfer to complete. This is the correct mental model for CEX-CEX arb: you need pre-funded accounts on both sides. The framework makes this a hard LP constraint rather than an afterthought.
- *Honest README*: calls out "fake opportunities" from stale orderbook data, withdrawal restrictions that invalidate routes, and lack of rigorous testing. Rare intellectual honesty.

**What's missing / broken:**
- Pre-funded account requirement means capital efficiency is low (money sitting idle on each exchange).
- CPLEX is proprietary. The LP formulation could be re-implemented with an open solver.
- Age: exchange APIs have changed; many pairs mentioned are no longer relevant.

**Steal:** The `consider_inter_exc_bal` mental model. For our CEX-CEX arb design: size maximum trade to whichever leg has the smaller pre-funded balance, not to the theoretical spread size.

---

## 6. solidquant/cex-dex-arb-research

**URL:** https://github.com/solidquant/cex-dex-arb-research  
**Stars:** 162 | **Language:** Python | **Halal posture:** Spot only, research tool only

**What it does:** Multi-orderbook aggregator that merges Binance + OKX bids/asks into a single `MultiOrderbook`, streams in real-time, and visualizes price spreads vs Uniswap V2/Sushiswap. Not an execution bot.

**Steal:** The `MultiOrderbook` merge pattern for combining orderbook snapshots from two CEXes into a single spread view — useful for our CEX-CEX divergence detector. Also: they normalize orderbook depth with a running VWAP calculation that's more robust to thin books than top-of-book alone.

---

## 7. ARBProtocol/solana-jupiter-bot

**URL:** https://github.com/ARBProtocol/solana-jupiter-bot  
**Stars:** 791 | **Last release:** v0.0.10-beta Sep 2022 | **Language:** JavaScript  
**Halal posture:** Spot only.

**Skip for code.** Issues reveal: slippage errors are frequent without a premium RPC, stablecoin pairs (USDC/USDT) are too competitive for retail, and the bot terminates after 100 consecutive swap errors to avoid spam costs. Lesson: without co-location + premium RPC, Jupiter-routed arb is not competitive. We already know this.

---

## 8. Ellipsis-Labs/phoenix-sdk

**URL:** https://github.com/Ellipsis-Labs/phoenix-sdk  
**Stars:** 99 | **Last commit:** unclear | **Language:** Rust + TypeScript + Python  
**Halal posture:** Spot CLOB, clean.

**What it does:** Official SDK for the Phoenix Legacy DEX. Provides Rust/TS/Python clients for submitting orders. No market-maker examples in the repo.

**Steal:** The Rust client's Red-Black tree traversal for top-of-book parsing — Phoenix's on-chain orderbook uses a sokoban tree. This is the missing piece in our Phoenix integration (currently we discover pools but parse zero reserves). Their `phoenix-sdk/rust/` deserializer is the reference implementation for our `mempool/parsers/phoenix.rs`.

---

## Patterns We're Missing vs the Field

**1. CEX-CEX pre-funded account model.** Every serious CEX-CEX arb framework (Hummingbot, hzjken) operates with pre-funded accounts on both sides, executing both legs simultaneously. We don't have a CEX-CEX execution path yet. Withdrawal time makes sequential transfer-then-arb non-viable. The design implication: if we add Bybit, we need a Bybit wallet with pre-funded USDT and the bot must leg into both sides within the same ~50ms window.

**2. Mainnet-fork Rust tests.** `0xNineteen` validates per-DEX swap math against forked mainnet state. We have unit tests but not against real account snapshots. This would have caught the Raydium tick-offset bug faster.

**3. Inventory skew vs binary gates.** Hummingbot's MM uses continuous inventory skew (shrink bids when long, shrink asks when short) rather than our binary ratio gate (block direction entirely at 90%). The continuous version captures more opportunities at the cost of slightly asymmetric sizing.

**4. Fill queue with retry.** Hummingbot's one-leg fill queue pattern (`_order_fill_buy_events` drained by `check_and_hedge_orders()` with blocked new orders until cleared) is more robust than our current single-leg inventory model. When we move to Model B (two-sided), we need this pattern.

**5. No halal-labeled projects exist.** Zero repos explicitly target halal compliance. Our constraint is self-imposed and differentiating only for us — no community to draw from on this specific dimension.

---

## Notable Failure Patterns Across the Field

- **Jito mempool shutdown (March 2024)**: Killed multiple bots that relied on `subscribe_mempool`. We avoided this by going Geyser-first.
- **JS integer overflow on lamport amounts**: `Number.MAX_SAFE_INTEGER` = 9e15 lamports = 9M SOL. Affects any JS bot doing raw lamport math on large pools. Rust i128 immune.
- **"Slippage error spam"**: Jupiter-routed bots that don't gate on RPC quality accumulate 100+ consecutive errors and self-terminate. Our `min_amount_out` enforcement + on-chain arb-guard is the correct approach.
- **Stale orderbook = fake opportunities**: Every cross-exchange framework documents this. Our TTL cache + on-chain guard is the right two-layer defense.
- **Funding rate arb (not halal)**: Several active repos (aoki-h-jp/funding-rate-arbitrage, kir1l/Funding-Arbitrage-Screener) target Binance/Bybit funding rate differentials. This is a perps strategy (riba-adjacent due to funding payments). Skip entirely.
