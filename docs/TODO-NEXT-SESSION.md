# Next Session TODO

## Issue 1: Mint fetch adds latency → pool state expires before simulation

The race condition fix works (mint programs are cached before router notification), but the ~100ms per mint fetch delays the PoolStateChange delivery. By the time the router runs the simulator, the pool state has expired from the 400ms TTL cache.

**Evidence:** 95 mints cached, 136 pools tracked, 0 opportunities (simulator rejects all as stale).

**Fix options (pick one):**
1. **Fire-and-forget mint fetch BUT skip first event** — spawn the fetch async, skip the first PoolStateChange for new mints, let the second event (with mints already cached) proceed normally. Most pools fire multiple events per second.
2. **Increase simulator TTL** — change `get()` TTL from 400ms to 2s. Pools are still "fresh enough" within 2 seconds.
3. **Fetch mints in parallel** — fetch both mints concurrently instead of sequentially (halves latency).

**Recommended: Option 1 + 3.** Fire-and-forget for first event, mints cache within ~100ms, second event proceeds with cached mints. Also fetch both mints in parallel with `tokio::join!`.

## Issue 2: Add compute budget IX
Bundles need priority fees for Jito auction placement.

## Issue 3: Simulate before submitting
After fixing the above, capture a tx and run `simulateTransaction` to verify it passes before live testing.

## Reference
- Searcher: `149xtHKerf2MgJVQ2CZB34bUALs8GaZjZWmQnC9si9yh` (0.75 SOL, untouched)
- 66 tests, 8 DEXes, Jito+Astralane relays
- The mint cache + Token-2022 detection is CORRECT — just needs the timing fixed
