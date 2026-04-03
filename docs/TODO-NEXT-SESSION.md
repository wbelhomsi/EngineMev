# Next Session TODO

## Status: BUNDLES ACCEPTED BY JITO + ASTRALANE. 22 Jito + 40 Astralane in 2 min.

## Immediate: Check If Bundles LANDED On-Chain
The bundles are ACCEPTED by relays — but accepted != landed. Check:
1. Jito Explorer: https://explorer.jito.wtf/bundle/{bundle_id}
2. Bundle IDs from last run: `30b2a59a...`, `0d1d5de2...`, `9fc4c484...`
3. If "landed" → first profit!
4. If "dropped" → faster searchers outbid us, or arb was already captured

## Review Findings to Address
- Nozomi tip accounts may need to be Nozomi-specific (not Jito's)
- bloXroute REST payload format needs API doc verification
- Astralane API key in URL should be header-only (redaction concern)
- Consider removing SIMULATE_BUNDLES from live runs (SIM FAILED is misleading — simulation can't create ATAs, but real relays accept the bundles fine)

## Architecture Summary
- Per-relay bundles: each relay owns tip+sign+send independently
- 5 relay modules: jito, astralane, nozomi, bloxroute, zeroslot
- bundle.rs returns Vec<Instruction> (no tips)
- Sanctum Shank IX: verified on-chain (29 SIM SUCCESS before per-relay refactor)
- 85 unit tests passing
- Balance: 0.75 SOL (untouched)

## Remaining Backlog
- Check bundle landing status (highest priority)
- Raydium AMM v4: enable after on-chain verification
- CLMM multi-tick crossing
- DLMM bin-by-bin simulation
- Address Lookup Tables for multi-hop routes
- Metrics/Prometheus
