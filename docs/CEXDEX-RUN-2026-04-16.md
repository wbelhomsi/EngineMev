# CEX-DEX Dry-Run Analysis — 2026-04-16

**Run duration:** 3599s (60 minutes)
**Start:** 2026-04-16T22:50:29 UTC  |  **End:** 2026-04-16T23:50:29 UTC
**Wallet:** 7.166 SOL, 0 USDC (ratio=1.0, SellOnDex-only)
**SOL/USDC price:** ~$89.00 (Binance bid/ask from WS)

## Raw counts

| Metric | Value |
|---|---|
| Detections | 25 |
| Sim profitable | 15 (60%) |
| Sim rejected | 10 (40%) |
| Submitted | 0 (dry_run) |
| Detections/min | 0.42 |
| **Gross USD profit (if all submitted)** | **$0.25** |

All 25 detections were on ONE pool: `Czfq3xZ...Ru44zE` (Orca Whirlpool SOL/USDC). The other 3 configured pools (two Orca alts + one Raydium AMM) never crossed the 5 bps spread threshold during this hour.

## Distributions

| Metric | p10 | p50 | p90 | max |
|---|---|---|---|---|
| Net profit USD | $0.012 | $0.016 | $0.021 | $0.021 |
| Trade size USD | $444 | $445 | $445 | $445 |
| Implied spread (bps, post-swap) | 1.1 | 1.5 | 1.9 | 1.9 |
| Tip (lamports) | — | 183,970 | 242,544 | 242,544 |

**Key observation:** median tip ≈ $0.016 at $89/SOL, which equals median net profit. **Tips are eating ~50% of gross profit.** This is because `TIP_FRACTION=0.50` applied to adjusted profit.

## Rejection reasons

All 10 rejections were the same class: `below threshold: net=$X < min=$0.0100`. So the bottleneck is the `MIN_PROFIT_USD` floor — marginal opportunities fail it.

## Limitations of this run

1. **One-sided inventory** — wallet was 100% SOL, so BuyOnDex was hard-capped. Need a balanced (50/50) inventory to test both directions.
2. **Thin sample** — 25 events is noisy. 4-8 hour runs would be much more reliable.
3. **One productive pool** — 3 of 4 configured pools never triggered. Either they're too tight, too illiquid, or we need more pools.
4. **Duplicate detection** — the detector fires every 50ms tick. Events cluster: 23:03 (×3), 23:16 (×3), 23:23 (×4), 23:30 (×5). Real unique opportunities ≈ 5, not 25. **Need time-based dedup per pool+direction.**
5. **Low volatility window** — SOL at $89 (post-correction flat period). Spreads were tiny (all <2 bps post-swap). Higher volatility = wider spreads.

## Recommended parameter changes

| Parameter | Current | Recommended | Rationale |
|---|---|---|---|
| `CEXDEX_MIN_SPREAD_BPS` | 5 | **3** | Only one pool hit 5 bps; lowering to 3 captures ~2-3x more candidates |
| `CEXDEX_MIN_PROFIT_USD` | 0.01 | **0.01** | Keep — p10 was $0.012, this correctly filters marginal trades |
| `CEXDEX_MAX_TRADE_SIZE_SOL` | 5.0 | **5.0** | All profitable trades hit this ceiling; increasing would add slippage |
| `CEXDEX_HARD_CAP_RATIO` | 0.80 | **0.80** | Not testable with one-sided inventory |
| `TIP_FRACTION` | 0.50 | **0.30** | Tips ate 50% of gross. 30% leaves more for us; still competitive for low-edge CEX-DEX |

## Required fixes before next run

1. **Fund wallet to ~50/50** (e.g., 2.5 SOL + $225 USDC) to enable BuyOnDex direction
2. **Add time-dedup** in the detector: skip if same (pool, direction) fired within the last 500ms
3. **Expand pool list** — add 2-3 more Orca Whirlpool SOL/USDC pools at different fee tiers + Meteora DLMM SOL/USDC. Top pools by volume can be found via Birdeye/DefiLlama
4. **Run longer** — 4 hours minimum during market hours (CEX is 24/7, but US session has more volatility)

## Verdict

**The pipeline works end-to-end.** Detection, simulation, tip calculation, stats collection, auto-shutdown — all functional. But the edge captured is thin ($0.25/hr gross ≈ $6/day) which is well below break-even at current tx costs. The economics only work if:

- Volatility spikes (spreads widen past 5 bps regularly)
- We expand pool coverage to catch more opportunities
- Tips come down (which they do during low-volume periods)

**Model A is probably marginal on a quiet day.** A better validation would be 24 hours of dry-run across different market conditions.
