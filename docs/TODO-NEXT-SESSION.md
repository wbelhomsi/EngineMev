# Next Session TODO

## Status: Engine production-ready. Jito + Astralane accepting bundles. Bundles not yet landing (auction competition).

## Key Metrics (last live run)
- 1,143 opportunities in 30 min
- 71 Jito bundles accepted, 109 Astralane accepted
- Zero format/decode errors
- Zero crashes, zero blockhash failures
- 85 unit tests passing
- Balance: 0.75 SOL (untouched — minimum_amount_out protects)

## Why Bundles Don't Land (404 on Jito Explorer)
Accepted by relay != landed on-chain. Our bundles enter the Jito auction but get outbid by faster searchers (co-located, <10ms latency vs our ~300ms).

## To Get First Landed Bundle

### Speed optimizations (highest impact)
1. **Skip simulator re-check** — rely on minimum_amount_out for safety. Saves ~100ms.
2. **Pre-compute common route templates** — have instructions ready, just swap amounts + sign
3. **Reduce tip_fraction to 50%** — currently 85%. Higher tip means we bid more but keep less. Try different fractions.
4. **Run 24/7** — less competitive hours (nights, weekends) may have easier arbs

### Structural improvements
5. **Address Lookup Tables (ALT)** — enables 3-hop routes, reduces tx size
6. **Raydium AMM v4** — enable in can_submit_route() (IX builder is done)
7. **Fix Jito write-lock rejection** — 40% of Jito submissions fail with "must write lock tip account". Not tx size (verified). Possibly Jito-side quirk with transaction message compilation.

### Investigate
8. **Check if any bundle TX appeared on-chain** — even as failed. Search signer pubkey on Solscan.
9. **Verify Nozomi/bloXroute tip accounts** — currently using Jito accounts, may need relay-specific ones.
10. **Speculative multi-route submission** — submit top 3-5 routes without simulation

## Architecture (completed this session)
- 9 DEX swap IX builders (all verified)
- Per-relay bundle architecture (5 independent relay modules)
- Sanctum Shank IX (1-byte discriminant, verified on-chain)
- Orca swap_v2 (15-account layout, verified)
- RPC flood protection (semaphore + bitmap cache + vault throttle)
- Arb dedup (max 5 per token path per 2s)
- Total tip accounting (single tip per relay, not summed)
- LazyLock static Pubkeys + O(1) pair index
