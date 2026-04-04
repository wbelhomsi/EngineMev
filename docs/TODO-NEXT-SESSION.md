# Next Session TODO

## Status: Engine fully functional. IX formats verified on Surfpool. Bundles accepted by relays. No profit landed — speed gap.

## What's Done
- 3 DEX swaps verified on Surfpool (Orca, Raydium CP, DLMM)
- Per-relay bundle architecture (5 independent relay modules)
- All 9 DEX IX builders implemented
- Sanctum Shank IX verified
- SOL-only routes + wSOL wrap/unwrap
- Token-2022 ATA resolution via RPC
- CLMM data size fixed (1544 bytes)
- CP authority PDA fixed
- SKIP_SIMULATOR flag for speed
- Surfpool E2E test infrastructure
- LstStateList entry layout corrected (mint at offset 0)
- 85 unit + 4 legacy e2e + 6 Surfpool E2E tests

## Why No Profit Yet
Bundles are accepted by Jito/Astralane but don't land:
1. **Speed gap**: ~300ms detection-to-submission vs <50ms for co-located searchers
2. **Sanctum rates hardcoded**: not real-time, produces inflated profit estimates
3. **Limited pool coverage**: CLMM (SqrtPriceLimitOverflow) and DAMM v2 (upgraded) disabled

## To Get First Profit (Priority Order)
1. **Co-location or faster RPC** — reduce latency to <100ms
2. **Fix CLMM SqrtPriceLimitOverflow** — investigate Raydium's actual bounds from their source code
3. **Real-time Sanctum rates** — call SOL value calculators via RPC to get current rates
4. **Speculative submission** — submit without waiting for route calculation to complete
5. **Run 24/7 during off-hours** — less competition nights/weekends

## Balance
0.749968 SOL (lost ~32,400 lamports from test txs, no on-chain swaps landed)

## Test Commands
```bash
# Unit tests
cargo test --test unit

# Surfpool E2E tests (requires RPC_URL)
RPC_URL=$RPC_URL cargo test --features e2e_surfpool --test e2e_surfpool -- --test-threads=1

# Live run (bundles only, no public tx)
MIN_PROFIT_LAMPORTS=1000 SKIP_SIMULATOR=true cargo run --release

# Live run with simulation logging
MIN_PROFIT_LAMPORTS=1000 SIMULATE_BUNDLES=true cargo run --release
```
