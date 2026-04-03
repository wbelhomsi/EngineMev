# Next Session TODO

## Status: All IX formats verified on-chain. First DLMM swap executed. Need real-time Sanctum rates.

## The Final Blocker: Hardcoded Sanctum Rates

Sanctum virtual pools use hardcoded rates (jitoSOL=1.082, mSOL=1.075, bSOL=1.06) that never update. The real on-chain rate changes continuously. With stale rates, SOL-base arb profits are ~684 lamports (below 100K min_profit threshold).

### Fix: Real-Time SOL Value from LstStateList

We already parse the LstStateList at startup (117 entries). Each entry has a `sol_value: u64` field at offset 8 within the 80-byte entry. This is the on-chain SOL value per LST token. 

**Plan:**
1. During bootstrap, read `sol_value` from each LstStateList entry (already have the data)
2. Compute rate as `sol_value / 10^9` (lamports to SOL ratio)
3. Update Sanctum virtual pool reserves using the real rate
4. Periodically re-fetch LstStateList (every 30s) to keep rates current
5. OR: subscribe to Sanctum pool reserve ATAs via Geyser for real-time updates

### Alternative: DEX↔DEX Arbs (No Sanctum)

Pure DEX arbs: SOL/USDC on Raydium vs Orca, SOL/TOKEN on different AMMs. These don't need Sanctum at all. The route calculator already finds them, but profits are tiny because spreads between DEXes on the same pair are razor-thin (<1 bps).

## Issues Fixed This Session

| Issue | Status |
|-------|--------|
| Sanctum Shank IX (1-byte discriminant) | Fixed, 29 SIM SUCCESS |
| Orca swap_v2 (15 accounts) | Fixed |
| Per-relay bundles (own tip+sign+send) | Fixed |
| wSOL wrap before first swap | Fixed |
| SOL-only route filter | Fixed |
| Non-SOL routes (can't execute) | Fixed |
| First DLMM swap on-chain | Hop 1 SUCCESS, hop 2 needs bitmap |

## Architecture Complete

- 85 unit tests, 5 relay modules, 9 DEX IX builders
- Jito + Astralane accepting bundles (71 + 109 in 30-min run)
- Balance: 0.75 SOL (untouched)
