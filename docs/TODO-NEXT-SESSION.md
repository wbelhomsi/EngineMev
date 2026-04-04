# Next Session TODO

## Status: Orca + Raydium CP + DLMM swaps VERIFIED on Surfpool. 4/6 E2E tests passing.

## Surfpool E2E Test Results

| Test | Status | Notes |
|------|--------|-------|
| `test_surfpool_starts` | ✅ PASS | Harness smoke test |
| `test_orca_whirlpool_swap` | ✅ PASS | SOL→USDC on Orca, verified on-chain |
| `test_raydium_cp_swap` | ✅ PASS | Fixed authority PDA seeds |
| `test_meteora_dlmm_swap` | ✅ PASS | Token-2022 ATA resolved via RPC |
| `test_raydium_clmm_swap` | ⏸ IGNORED | SqrtPriceLimitOverflow — needs Raydium-specific bounds |
| `test_meteora_damm_v2_swap` | ⏸ IGNORED | AccountNotEnoughKeys — program may have been upgraded |

Run: `RPC_URL=$RPC_URL cargo test --features e2e_surfpool --test e2e_surfpool -- --test-threads=1`

## Fixes Applied This Session

### Critical
- **Raydium CLMM data size**: 1544 bytes on mainnet (was matching only 1560 → missed ALL CLMM pools)
- **Raydium CP authority PDA**: seeds=`["vault_and_lp_mint_auth_seed"]` (was empty `[]`)
- **Token-2022 ATA resolution**: use RPC `getAccountInfo` owner as authoritative source
- **SOL-only route filter**: only routes starting/ending with SOL (we only hold SOL)
- **wSOL wrap/unwrap**: system_instruction::transfer + SyncNative before, CloseAccount after
- **Per-relay bundles**: each relay owns tip+sign+send independently
- **Sanctum Shank IX**: 1-byte discriminant, 27-byte data, 12+variable accounts

### Infrastructure
- Surfpool E2E test harness with subprocess lifecycle management
- 5 per-DEX swap tests (3 passing, 2 ignored with known issues)
- 85 unit tests + 4 legacy e2e tests + 4 Surfpool E2E tests

## Remaining Work

### To fix ignored tests
1. **CLMM**: Investigate Raydium's actual MIN/MAX_SQRT_PRICE_X64 constants
2. **DAMM v2**: Check if program was upgraded with new required accounts

### To get first profitable trade
1. Run with Orca + Raydium CP + DLMM routes only (proven DEXes)
2. Focus on SOL-base routes with real Geyser-updated pool state
3. Check if Jito-submitted bundles land (we know IX format is correct now)

### Nice to have
- Pipeline e2e tests (2-hop arb roundtrip, Token-2022 ATA, wSOL cycle)
- Phoenix + Manifest e2e tests
- Sanctum e2e tests
- Address Lookup Tables for multi-hop routes
