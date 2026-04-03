# Next Session TODO

## Status: Engine runs on mainnet. 136 opportunities in ~10 min. Bundles building but mostly rate-limited.

## Issues Found During 10-Min Live Run (2026-04-03)

### CRITICAL: 124 of 135 bundles submitted to 0 relays (rate limiter too aggressive)
Most bundles fire during a burst (all from same pool state change) and the rate limiter blocks all but the first. Only 11 bundles actually reached any relay. The per-relay rate limiter is working correctly, but the BURST of 130+ opportunities from a single event means only 1 gets through.

**Root cause:** Dedup issue — 131 of 136 opportunities have identical profit (1,794,666 lamports). These are the SAME arb opportunity detected across different pool addresses for the same token pair. The route calculator finds the same price dislocation through every pool that trades the pair.

**Fix:** Dedup opportunities by (base_mint, intermediate_mint) pair BEFORE submission. Only submit the most profitable route per pair per slot.

### HIGH: 2,392 DLMM bitmap check failures + 720 vault fetch failures
Helius RPC is being hammered by fire-and-forget bitmap checks and vault fetches. Every DLMM pool state update triggers a bitmap PDA existence check via RPC. Every Raydium AMM/CP update triggers a vault balance fetch.

**Fix options:**
1. Cache bitmap existence permanently (it either exists or doesn't — won't change)
2. Batch vault fetches instead of one-per-pool
3. Rate limit RPC calls to stay within Helius free tier

### MEDIUM: 76 blockhash fetch failures
RPC rate limiting cascades — vault/bitmap fetches consume the RPC quota, leaving blockhash fetches to fail. When blockhash is stale, ALL opportunities get skipped.

**Fix:** Use a separate RPC endpoint for blockhash (or prioritize it). Blockhash is the most critical RPC call.

### MEDIUM: 7 Geyser disconnects in 10 minutes
LaserStream connection drops every ~90 seconds. Reconnect works (1s backoff), but each disconnect loses ~1-2 seconds of data.

**Fix:** Check if this is a LaserStream plan limit. May need to upgrade Helius plan or use a different Geyser provider.

### MEDIUM: Jito rate limit (-32097) hit 3 times
"Network congested. Endpoint is globally rate limited." — this is the unauth rate limit (1 bundle/sec). Need Jito UUID approval for higher limits.

### LOW: All opportunities use same profit amount (no dedup)
131 opportunities with gross=1,794,666 and 4 with gross=6,736,352. The engine finds the same arb through dozens of different pool paths. This is wasted computation and relay bandwidth.

## Key Metrics From Run
- 1,196 pools tracked
- 136 opportunities detected
- 135 bundles built
- 11 bundles reached a relay
- 3 Jito rejections (rate limited)
- Tip accounting working: total_tip=1,445,999 (jito=1,345,999, astralane=100,000)
- Balance: untouched (revert protection + min_amount_out)

## Priority for Next Session
1. **Opportunity deduplication** (biggest bang — 10x fewer bundles, 10x more relay throughput)
2. **Cache DLMM bitmap existence** (eliminate 2,392 unnecessary RPC calls)
3. **Batch/throttle vault fetches** (eliminate 720 RPC calls)
4. **Separate RPC for blockhash** (ensure blockhash never stale)
5. **Jito UUID** (higher rate limits)
