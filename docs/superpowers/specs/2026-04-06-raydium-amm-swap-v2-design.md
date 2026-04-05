# Raydium AMM v4 Swap V2 Migration

**Date:** 2026-04-06
**Status:** Approved

## Goal

Replace the legacy 18-account Raydium AMM v4 `swap_base_in` (discriminator 9) with the 8-account `swap_base_in_v2` (discriminator 16). Removes all OpenBook/Serum dependencies, saving 320 bytes per hop.

## Background

Raydium deployed `SwapBaseInV2` in September 2025. It removes all 10 Serum/OpenBook accounts from the swap instruction. The old V1 instruction is deprecated but still functional. All actively traded pools support V2.

## Changes

### 1. Bundle builder: `build_raydium_amm_swap_ix` (src/executor/bundle.rs)

**Before (V1, 18 accounts):**
```
discriminator = 9
accounts: SPL Token, amm_id, amm_authority, open_orders, target_orders,
  coin_vault, pc_vault, serum_program, serum_market, serum_bids, serum_asks,
  serum_event_queue, serum_coin_vault, serum_pc_vault, serum_vault_signer,
  user_source, user_dest, signer
```

**After (V2, 8 accounts):**
```
discriminator = 16
accounts: SPL Token, amm_id, amm_authority, coin_vault, pc_vault,
  user_source, user_dest, signer
```

**Data format:** Unchanged — `[discriminator(1), amount_in(8), min_out(8)]` = 17 bytes.

**amm_authority:** PDA from `create_program_address(&[&[nonce]], &amm_program)` where nonce is from pool state byte 8. Most pools use nonce=254 → authority = `5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1`. This is already in the ALT.

**What's removed from the function:**
- No longer needs: `open_orders`, `target_orders`, `market_id`, `market_program`, `serum_bids`, `serum_asks`, `serum_event_queue`, `serum_coin_vault`, `serum_pc_vault`, `serum_vault_signer_nonce`
- Still needs: `vault_a`, `vault_b`, `amm_nonce` (for authority PDA)

### 2. Stream parser: remove Serum lazy fetch (src/mempool/stream.rs)

The current code has a lazy fetch path: when a Raydium AMM v4 pool state change comes in, it spawns an async task to `getMultipleAccounts` on the Serum market to fetch bids/asks/event_queue/coin_vault/pc_vault addresses. This is no longer needed.

**Remove:**
- The Serum market account fetching logic
- The `serum_bids`, `serum_asks`, `serum_event_queue`, `serum_coin_vault`, `serum_pc_vault`, `serum_vault_signer_nonce` fields from `PoolExtra` writes in the Raydium AMM path

**Keep:**
- Pool state parsing (address, mints, vaults, nonce, open_orders, market_id) — still needed for pool identification
- The `PoolExtra` fields themselves (other code may reference them) — just stop populating the Serum-specific ones

### 3. PoolExtra cleanup (src/router/pool.rs)

Remove the Serum-specific fields from `PoolExtra`:
- `serum_bids: Option<Pubkey>`
- `serum_asks: Option<Pubkey>`
- `serum_event_queue: Option<Pubkey>`
- `serum_coin_vault: Option<Pubkey>`
- `serum_pc_vault: Option<Pubkey>`
- `serum_vault_signer_nonce: Option<u64>`

### 4. Test updates

**TDD approach — write failing tests first:**

**New test: `test_raydium_amm_v4_swap_v2_8_accounts`**
- Verify the V2 IX has exactly 8 accounts
- Verify discriminator is 16 (not 9)
- Verify account ordering: SPL Token, amm, authority, coin_vault, pc_vault, user_source, user_dest, signer

**New test: `test_raydium_amm_v4_swap_v2_no_serum_required`**
- Build IX with a PoolExtra that has `vault_a`, `vault_b`, `amm_nonce` but NO serum fields
- Verify it succeeds (V1 would return None without serum fields)

**Update existing tests:**
- `test_raydium_amm_v4_swap_ix_18_accounts` → rename to `test_raydium_amm_v4_swap_ix_8_accounts`, update assertion
- `test_raydium_amm_v4_swap_ix_returns_none_without_serum` → delete (no longer applies)
- `test_raydium_amm_v4_swap_ix_returns_none_without_nonce` → keep (nonce still needed)
- `test_raydium_amm_v4_swap_ix_returns_none_without_open_orders` → delete (open_orders no longer needed for V2)

**Surfpool e2e test:**
- Update `tests/e2e_surfpool/common.rs` to remove Serum fetch helper
- The Raydium AMM v4 swap test should work with fewer accounts

## Files Modified

| File | Change |
|------|--------|
| `src/executor/bundle.rs` | Replace `build_raydium_amm_swap_ix` — 8 accounts, discriminator 16 |
| `src/mempool/stream.rs` | Remove lazy Serum market account fetching |
| `src/router/pool.rs` | Remove Serum fields from PoolExtra |
| `tests/unit/bundle_raydium_amm.rs` | TDD: update tests for V2 |
| `tests/e2e_surfpool/common.rs` | Remove Serum fetch helper |

## Impact

- **TX size:** 10 fewer accounts × 32 bytes = 320 bytes saved per Raydium AMM hop
- **Latency:** Eliminates 1 RPC call (Serum market fetch) per new AMM pool
- **Complexity:** Removes ~100 lines of Serum fetching code
- **Risk:** Very low — V2 has been live since September 2025, all active pools support it

## Non-Goals

- Raydium CP or CLMM changes (different programs, already efficient)
- Multi-DEX arb-guard CPI (separate spec)
