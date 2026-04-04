# Next Session TODO

## IMMEDIATE: Write implementation plan + implement Surfpool E2E tests

**Spec ready:** `docs/superpowers/specs/2026-04-04-surfpool-e2e-tests-design.md`

1. Invoke `superpowers:writing-plans` on the spec to create the implementation plan
2. Implement the test harness (Surfpool lifecycle management)
3. Find and hardcode known pool addresses for each DEX type
4. Implement per-DEX swap tests (8 tests)
5. Implement pipeline tests (2-hop arb, wSOL wrap/unwrap, Token-2022)
6. Use tests to fix remaining DLMM bitmap extension issue

## Surfpool Installed
- Version: 1.1.2
- Start: `NO_DNA=1 surfpool start --rpc-url $RPC_URL --ci --port 18900 --airdrop <signer> --no-deploy`

## Bugs Fixed This Session
- Token-2022 ATA mismatch: ATA creation now uses pool token programs (verified on Surfpool)
- wSOL wrap/unwrap: added system_instruction::transfer + SyncNative + CloseAccount
- SOL-only route filter: only routes starting/ending with SOL
- SOL-base route search: calculator always searches SOL as base
- Simulator TTL: uses get_any() so Sanctum virtual pools don't expire
- Per-relay bundle architecture: each relay owns tip+sign+send
- min_final_output: no longer subtracts tip (tip is separate IX)

## Remaining Bugs
- DLMM bitmap extension: some pools need it, don't have it on-chain → filter these pools
- Sanctum virtual pool rates: hardcoded, need real-time sol_value from LstStateList

## Architecture
- 85 unit tests, 5 relay modules, 9 DEX IX builders
- Surfpool for local E2E testing
- Balance: ~0.7499 SOL
