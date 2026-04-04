# CLMM Fix + Real-Time LST Rates

**Date:** 2026-04-04
**Status:** Approved

## Part 1: CLMM SqrtPriceLimitOverflow Fix

Set `sqrt_price_limit = 0u128` in `build_raydium_clmm_swap_ix()`. The on-chain program substitutes its own correct MIN+1/MAX-1 constants and determines direction from the input vault mint (swap_v2.rs lines 153-158).

**File:** `src/executor/bundle.rs` — one line change.

**Verification:** `test_raydium_clmm_swap` Surfpool E2E test should pass.

## Part 2: Real-Time LST Rates

### Startup: RPC Fetch

New `fetch_lst_rates()` async function:
- `getMultipleAccounts` for Jito + BlazeStake pools (dataSlice offset=258, length=16)
- `getAccountInfo` for Marinade State (dataSlice offset=512, length=8)
- Parse: jitoSOL/bSOL rate = total_lamports / pool_token_supply, mSOL rate = msol_price / 2^32
- Update Sanctum virtual pool reserves in StateCache

Called after `bootstrap_sanctum_pools()` in main.rs.

### Runtime: Geyser Subscription

Add 3 stake pool accounts to Geyser subscription:
- `Jito4APyf642JPZPx3hGc6WWJ8zPKtRbRs4P815Awbb`
- `stk9ApL5HeVAwPLr3TLhDXdZS8ptVu7zp6ov8HFDuMi`
- `8szGkuLTAux9XMgZ2vtY39jVSowEcpBfFfD8hXSEqdGC`

In `process_update`, detect by account address. On update, re-parse rate and update virtual pool.

### Byte Offsets (Verified on Mainnet)

| Account Type | Field | Offset | Size |
|-------------|-------|--------|------|
| SPL Stake Pool | total_lamports | 258 | 8 bytes (u64 LE) |
| SPL Stake Pool | pool_token_supply | 266 | 8 bytes (u64 LE) |
| Marinade State | msol_price | 512 | 8 bytes (u64 LE, divide by 2^32) |

### Files Changed

- `src/executor/bundle.rs` — CLMM sqrt_price_limit = 0
- `src/mempool/stream.rs` — add stake pool accounts to Geyser sub + handle updates
- `src/main.rs` — fetch_lst_rates() at startup, update hardcoded fallbacks to ~April 2026 values
- `src/config.rs` — stake pool offset constants

### Updated Fallback Rates

| LST | Old Rate | New Rate (April 2026) |
|-----|----------|----------------------|
| jitoSOL | 1.082 | 1.271 |
| mSOL | 1.075 | 1.371 |
| bSOL | 1.060 | 1.286 |
