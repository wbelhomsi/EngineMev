# Next Session TODO

## Status: 3 DEX swaps verified on Surfpool. Engine runs live with bundle submissions. No profit landed yet.

## Surfpool E2E Tests: 4 PASSED, 2 IGNORED

| Test | Status | Notes |
|------|--------|-------|
| Harness smoke | ✅ PASS | |
| Orca Whirlpool | ✅ PASS | SOL→USDC verified on-chain |
| Raydium CP | ✅ PASS | Fixed authority PDA seeds |
| Meteora DLMM | ✅ PASS | Token-2022 ATA via RPC resolution |
| Raydium CLMM | ⏸ IGNORED | SqrtPriceLimitOverflow — wrong bounds or direction issue |
| DAMM v2 | ⏸ IGNORED | AccountNotEnoughKeys — program upgraded |

Run: `RPC_URL=$RPC_URL cargo test --features e2e_surfpool --test e2e_surfpool -- --test-threads=1`

## Why No Profit Yet

Bundles are accepted by Jito/Astralane but don't land on-chain:
1. **Speed** — ~300ms from detection to submission, faster searchers outbid us
2. **Small profits** — 15K lamport opportunities are below competitive tip thresholds
3. **Stale rates** — Sanctum virtual pool rates are hardcoded, not real-time
4. **Limited DEXes** — CLMM and DAMM v2 disabled, reducing pool coverage

## To Get First Profit

1. **Fix CLMM** — investigate SqrtPriceLimitOverflow, verify bounds against Raydium source
2. **Real-time Sanctum rates** — fetch sol_value from LstStateList periodically
3. **Reduce latency** — skip simulator, submit immediately after route found
4. **Increase input** — use more SOL per swap for larger profits
5. **Try off-hours** — less competition at night/weekends

## Critical Bugs Fixed This Session
- CLMM data size: 1544 bytes (was 1560 — missed ALL CLMM pools)
- CP authority PDA: seeds=["vault_and_lp_mint_auth_seed"]
- Token-2022 ATA: RPC-based mint owner resolution
- SOL-only routes + wSOL wrap/unwrap
- Per-relay bundles (each relay owns tip+sign+send)
- min_final_output doesn't subtract tip (tip is separate IX)
- Sanctum Shank IX (1-byte discriminant, verified 29 SIM SUCCESS)

## Test Coverage
- 85 unit tests (all passing)
- 4 legacy e2e tests (all passing)
- 6 Surfpool e2e tests (4 passing, 2 ignored)
- Surfpool 1.1.2 installed for local testing
