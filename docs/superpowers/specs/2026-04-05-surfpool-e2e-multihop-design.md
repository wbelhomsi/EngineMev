# Surfpool E2E: Multi-Hop Arb + Raydium AMM v4

**Date:** 2026-04-05
**Status:** Approved

## Goal

Two new Surfpool e2e tests that execute real on-chain transactions against forked mainnet state. No mocking.

## Test 1: Multi-hop arb via BundleBuilder (SOL→USDC→SOL)

Uses the production `BundleBuilder::build_arb_instructions()` code path.

**Pools:**
- Leg 1: Orca Whirlpool SOL/USDC (`HJPjoWUrhoZzkNfRpHuieeFk9WcZWjwy6PBjZ81ngndJ`)
- Leg 2: Raydium CLMM SOL/USDC (`2JtkunkYCRbe5YZuGU6kLFmNwN22Ba1pCicHoqW5Eqja`)

**Flow:**
1. Harness starts Surfpool, forks mainnet
2. Fetch both pool accounts from Surfpool RPC
3. Parse via existing `parse_orca_whirlpool` and `parse_raydium_clmm`
4. Populate a `StateCache` with both pools
5. Construct `ArbRoute` manually: SOL →[Orca]→ USDC →[CLMM]→ SOL (2 hops)
6. Call `BundleBuilder::build_arb_instructions(&route, min_output)`
7. If IX building succeeds, wrap with wSOL setup/teardown and send to Surfpool
8. Accept either: TX success (arb profitable) or on-chain error (price not favorable, slippage). Both validate the IX building. Only panic on build failures.

**What this validates:**
- Production bundle builder produces valid multi-hop instructions
- Orca + Raydium CLMM IX account layouts are correct for on-chain execution
- wSOL wrapping/unwrapping between hops works

## Test 2: Raydium AMM v4 single swap

The only DEX with zero on-chain test coverage. Complex 18-account IX with Serum/OpenBook market accounts.

**Pool:** Need to find and register a Raydium AMM v4 SOL/X pool in `known_pools()`.

**Flow:**
1. Add AMM v4 pool to `known_pools()` registry
2. Add `DexType::RaydiumAmm` case to `parse_pool()` and `build_swap_ix()` in `common.rs`
3. Fetch pool data, parse with `parse_raydium_amm_v4`
4. Lazy-fetch Serum market accounts via harness RPC (market address from pool state → fetch bids/asks/event_queue/base_vault/quote_vault)
5. Build swap IX via `build_raydium_amm_v4_swap_ix`
6. Send to Surfpool, verify execution

**Challenge:** The Serum market account addresses are derived from the pool's `market_id` field. The harness needs RPC helpers to fetch these 5+ accounts. If Surfpool doesn't auto-fork them, the test will surface this as a clear error.

## Files Modified

| File | Change |
|------|--------|
| `tests/e2e_surfpool/common.rs` | Add Raydium CLMM SOL/USDC + Raydium AMM v4 pools to `known_pools()`, add AMM v4 parse/build support |
| `tests/e2e_surfpool/dex_swaps.rs` | Add `test_raydium_amm_v4_swap()` |
| `tests/e2e_surfpool/pipeline.rs` | Add `test_multihop_arb_orca_clmm()` |

## Non-Goals

- Testing profitability (on-chain prices fluctuate)
- Testing relay submission (requires real relay endpoints)
- Fixture-based account injection (we fork mainnet directly)
