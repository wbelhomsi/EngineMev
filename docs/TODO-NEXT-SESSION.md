# Next Session TODO

## Status: Per-relay architecture complete. Sanctum IX verified (29 SIM SUCCESS). Need live test of relay submissions.

## Immediate: Live Verification
1. Build release and run with `SIMULATE_BUNDLES=true` for 2-5 min
2. Check: "could not be decoded" errors should be GONE (each tx now has only 1 tip)
3. Check: SIM SUCCESS count should be similar to before (~29 in 90s)
4. Check: Jito/Astralane should ACCEPT bundles (not just our simulation)
5. If accepted: check Jito Explorer for bundle landing status

## Architecture Changes This Session
- Per-relay bundle architecture: each relay owns tip+sign+send independently
- 5 relay modules: jito.rs, astralane.rs, nozomi.rs, bloxroute.rs, zeroslot.rs
- bundle.rs returns Vec<Instruction> (no tips, no signing)
- Simulator uses single tip (not sum of all relay tips)
- Sanctum Shank IX: 1-byte discriminant, 27-byte data, 12+variable accounts
- LstStateList bootstrapped at startup (117 LSTs indexed)
- Orca swap_v2: 15 accounts (was 12)
- 85 unit tests passing

## If Bundles Still Not Landing
- Check if `minimum_amount_out` is too aggressive (try lower tip_fraction)
- Check timing: how many slots between detection and submission?
- Consider removing `minimum_amount_out` on a test run to see if txs would succeed
- ALT (Address Lookup Tables) for further tx size reduction

## Remaining Backlog
- Raydium AMM v4: enable in can_submit_route() after on-chain verification
- CLMM multi-tick crossing (underestimates large swaps)
- DLMM bin-by-bin simulation (synthetic reserves approximate)
- Metrics/Prometheus endpoint
- Speculative multi-route submission (skip simulation, submit top N)
