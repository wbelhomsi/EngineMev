# Geyser Stream Fix: Parse Pool State Accounts, Not Token Vaults

## Problem

The Geyser subscription in `stream.rs` watches accounts owned by DEX programs (Raydium, Orca, Meteora). These are **pool state accounts** (AmmInfo, Whirlpool, LbPair), not SPL Token vaults. But `process_update` parses bytes 64..72 as an SPL Token balance — completely wrong layout. Result: garbage data, no useful events reach the router.

## Solution

Replace the single SPL Token parser with per-DEX pool state parsers. When a pool state account update arrives via Geyser, identify which DEX owns it (by subscription filter key), parse the pool-specific layout, and update the PoolState in the StateCache.

## Supported DEXes (6 total)

| DEX | Program ID | Data Size | Reserves Source |
|-----|-----------|-----------|----------------|
| Orca Whirlpool | `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc` | 653 | Pool state (sqrt_price, tick, liquidity) |
| Meteora DLMM | `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo` | 904 | Pool state (active_id, bin_step) |
| Meteora DAMM v2 | `cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG` | 1112 | Pool state (token_a_amount, token_b_amount) |
| Raydium CLMM | `CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK` | 1560 | Pool state (sqrt_price_x64, tick_current, liquidity) |
| Raydium CP | `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C` | 637 | Vaults (lazy fetch on pool state change) |
| Raydium AMM v4 | `675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8` | 752 | Vaults (lazy fetch on pool state change) |

### Two Categories

**Category A — Self-contained (4 DEXes):** Orca, Meteora DLMM, Meteora DAMM v2, Raydium CLMM. Pool state contains everything needed for price/reserve calculation. No vault lookups required. Geyser pool state update → parse → update cache → done.

**Category B — Vault-dependent (2 DEXes):** Raydium AMM v4 and Raydium CP. Pool state gives us vault pubkeys and fee accumulators, but actual reserves live in SPL Token vault accounts. When Geyser fires a pool state update (meaning a swap happened), we do a lazy vault fetch: `getMultipleAccounts` on the 2 vaults with `dataSlice: {offset: 64, length: 8}` (just the balance, 16 bytes total). ~1 RPC call per swap event.

---

## Subscription (expanded)

```
dex_0: Raydium AMM v4  (675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8)
dex_1: Raydium CLMM    (CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK)
dex_2: Raydium CP       (CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C)
dex_3: Orca Whirlpool   (whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc)
dex_4: Meteora DLMM     (LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo)
dex_5: Meteora DAMM v2  (cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG)
dex_6: Sanctum           (5ocnV1qiCgaQR8Jb8xWnVbApfaygJ8tNoZfgPwsgx9kx)
```

---

## Per-DEX Parsing — Account Layouts

### Orca Whirlpool (653 bytes, Anchor)

| Offset | Field | Type | Size |
|--------|-------|------|------|
| 0 | discriminator | [u8;8] | 8 |
| 49 | liquidity | u128 | 16 |
| 65 | sqrt_price | u128 | 16 |
| 81 | tick_current_index | i32 | 4 |
| 101 | token_mint_a | Pubkey | 32 |
| 133 | token_vault_a | Pubkey | 32 |
| 181 | token_mint_b | Pubkey | 32 |
| 213 | token_vault_b | Pubkey | 32 |

**Reserve derivation:** For constant-product approximation: `reserve_a = liquidity / sqrt_price`, `reserve_b = liquidity * sqrt_price` (scaled from Q64.64). Vaults not needed for pricing.

### Raydium CLMM (1560 bytes, Anchor, packed)

| Offset | Field | Type | Size |
|--------|-------|------|------|
| 0 | discriminator | [u8;8] | 8 |
| 73 | token_mint_0 | Pubkey | 32 |
| 105 | token_mint_1 | Pubkey | 32 |
| 137 | token_vault_0 | Pubkey | 32 |
| 169 | token_vault_1 | Pubkey | 32 |
| 237 | liquidity | u128 | 16 |
| 253 | sqrt_price_x64 | u128 | 16 |
| 261 | tick_current | i32 | 4 |

**Reserve derivation:** Same CLMM math as Orca — derive from sqrt_price_x64 + liquidity. Vaults not needed for pricing.

### Meteora DLMM (904 bytes, Anchor)

| Offset | Field | Type | Size |
|--------|-------|------|------|
| 0 | discriminator | [u8;8] | 8 |
| 76 | active_id | i32 | 4 |
| 80 | bin_step | u16 | 2 |
| 88 | token_x_mint | Pubkey | 32 |
| 120 | token_y_mint | Pubkey | 32 |
| 152 | reserve_x (vault) | Pubkey | 32 |
| 184 | reserve_y (vault) | Pubkey | 32 |

**Reserve derivation:** Price = `(1 + bin_step/10000)^(active_id - 2^23)`. Synthetic reserves set to produce this price ratio with large values (same as Sanctum virtual pools).

### Meteora DAMM v2 (1112 bytes, Anchor)

| Offset | Field | Type | Size |
|--------|-------|------|------|
| 0 | discriminator | [u8;8] | 8 |
| 168 | token_a_mint | Pubkey | 32 |
| 200 | token_b_mint | Pubkey | 32 |
| 232 | token_a_vault | Pubkey | 32 |
| 264 | token_b_vault | Pubkey | 32 |
| 360 | liquidity | u128 | 16 |
| 456 | sqrt_price | u128 | 16 |
| 481 | pool_status | u8 | 1 |
| 680 | token_a_amount | u64 | 8 |
| 688 | token_b_amount | u64 | 8 |

**Reserves:** Directly in pool state — `token_a_amount` at 680, `token_b_amount` at 688. No vault fetch needed.

### Raydium CP (637 bytes, Anchor, packed)

| Offset | Field | Type | Size |
|--------|-------|------|------|
| 0 | discriminator | [u8;8] | 8 |
| 72 | token_0_vault | Pubkey | 32 |
| 104 | token_1_vault | Pubkey | 32 |
| 168 | token_0_mint | Pubkey | 32 |
| 200 | token_1_mint | Pubkey | 32 |
| 329 | status | u8 | 1 |
| 341 | protocol_fees_token_0 | u64 | 8 |
| 349 | protocol_fees_token_1 | u64 | 8 |
| 357 | fund_fees_token_0 | u64 | 8 |
| 365 | fund_fees_token_1 | u64 | 8 |
| 397 | creator_fees_token_0 | u64 | 8 |
| 405 | creator_fees_token_1 | u64 | 8 |

**Reserves:** `vault_balance - protocol_fees - fund_fees - creator_fees`. Requires lazy vault fetch on pool state change.

### Raydium AMM v4 (752 bytes, no Anchor)

| Offset | Field | Type | Size |
|--------|-------|------|------|
| 0 | status | u64 | 8 |
| 192 | baseNeedTakePnl | u64 | 8 |
| 200 | quoteNeedTakePnl | u64 | 8 |
| 336 | coin_vault | Pubkey | 32 |
| 368 | pc_vault | Pubkey | 32 |
| 400 | coin_vault_mint | Pubkey | 32 |
| 432 | pc_vault_mint | Pubkey | 32 |

**Reserves:** `vault_balance - need_take_pnl`. Requires lazy vault fetch on pool state change.

---

## Event Processing Flow

```
Geyser account update arrives
  → identify DEX from filter key (dex_0..dex_6)
  → validate data size matches expected pool account
  → dispatch to per-DEX parser:

  Category A (Orca, CLMM, DLMM, DAMM v2):
    → parse pool state fields
    → derive/read reserves
    → StateCache.upsert(pool_address, PoolState)
    → send PoolStateChange{pool_address, slot} to router

  Category B (Raydium AMM v4, Raydium CP):
    → parse vault pubkeys + fee fields from pool state
    → async: getMultipleAccounts on 2 vaults (dataSlice: offset 64, length 8)
    → reserves = vault_balance - fees
    → StateCache.upsert(pool_address, PoolState)
    → send PoolStateChange{pool_address, slot} to router
```

### Lazy Vault Fetch for Category B

When a Raydium AMM v4 or CP pool state update arrives:
1. Parse vault pubkeys from pool data
2. Fire async `getMultipleAccounts` call with `dataSlice: {offset: 64, length: 8}` (just the u64 balance)
3. Calculate effective reserves: `vault_balance - accumulated_fees`
4. Update the PoolState in cache with fresh reserves

This adds ~10-50ms latency for Raydium pools but avoids bootstrapping 1.4M vault balances upfront. The RPC call is tiny (2 accounts, 16 bytes response).

To avoid overwhelming the RPC, deduplicate: if the same pool fires multiple Geyser events within a short window, only fetch once. Use a simple `DashSet<Pubkey>` of "pending vault fetches" with a 100ms cooldown.

---

## PoolStateChange Redesign

```rust
pub struct PoolStateChange {
    pub pool_address: Pubkey,
    pub slot: u64,
}
```

The router reads the full PoolState from cache (which was updated by the stream before sending the event). No balance data in the event itself.

## Router Changes

Simplified — no more vault→pool lookup:
1. Receive `PoolStateChange { pool_address, slot }`
2. Read pool from cache (already updated)
3. Build trigger for route discovery
4. Find routes + simulate

## DexType Expansion

Add new variants:
```rust
pub enum DexType {
    RaydiumAmm,
    RaydiumClmm,
    RaydiumCp,         // NEW
    OrcaWhirlpool,
    MeteoraDlmm,
    MeteoraDammV2,     // NEW
    SanctumInfinity,
}
```

## Bootstrap Changes

- **Remove:** Orca, Meteora DLMM, Meteora DAMM v2 bootstrap — Geyser gives us everything, pools self-register on first update
- **Remove:** Raydium CLMM bootstrap — same reason
- **Keep:** Raydium AMM v4 and Raydium CP bootstrap — needed to discover vault pubkeys for initial pool→vault mapping. But this runs in background (non-blocking) and is only needed so the lazy vault fetch knows which vaults to query.
- **Actually simplify further:** We don't even need to bootstrap Raydium AMM/CP upfront. When a Geyser event arrives for a Raydium pool, we parse the vault pubkeys from the pool state data right then. First event = discover pool + fetch vaults. No bootstrap needed at all.

**Result: Zero startup bootstrap. All pools discovered lazily via Geyser.**

## Config Changes

Add to `config.rs` programs module:
```rust
pub fn raydium_cp() -> Pubkey { ... }       // CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C
pub fn meteora_damm_v2() -> Pubkey { ... }  // cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG
```

Update `monitored_programs()` to include all 6 DEXes + Sanctum.

## Files Changed

| File | Action | What |
|------|--------|------|
| `src/mempool/stream.rs` | Rewrite | Per-DEX pool state parsing, direct cache update, lazy vault fetch for Raydium |
| `src/mempool/mod.rs` | Modify | Update PoolStateChange |
| `src/router/pool.rs` | Modify | Add `RaydiumCp`, `MeteoraDammV2` to DexType, add `base_fee_bps()` |
| `src/config.rs` | Modify | Add raydium_cp(), meteora_damm_v2() program IDs, update monitored_programs() |
| `src/state/cache.rs` | Modify | Remove vault_to_pool index (no longer needed), remove vault balance update methods |
| `src/state/bootstrap.rs` | Remove or gut | Bootstrap no longer needed — all pools discovered via Geyser |
| `src/main.rs` | Modify | Remove bootstrap call, simplify router loop (no vault lookup) |
| `src/executor/bundle.rs` | Modify | Add match arms for new DexType variants |
| `tests/unit/stream_parsing.rs` | Create | Unit tests for per-DEX Geyser event parsing |
| `tests/unit/mod.rs` | Modify | Add new test module |
| `tests/e2e/lst_pipeline.rs` | Modify | Update for new PoolStateChange |

## Testing

- **Unit tests:** For each DEX, construct raw account data bytes, feed through parser, verify PoolState fields
- **E2E:** Update existing tests for new PoolStateChange format
- **Manual:** Run with DRY_RUN=true against Helius LaserStream, verify all 6 DEXes produce events and opportunities log

## Risk Notes

- Raydium AMM v4 `baseNeedTakePnl` offset (192) and `quoteNeedTakePnl` (200) confirmed by prior analysis but should be verified on mainnet
- Raydium CP fee offsets (341-413) from source code — verify on mainnet
- Meteora DAMM v2 `token_a_amount`/`token_b_amount` at 680/688 from source code — verify on mainnet
- CLMM reserve derivation from sqrt_price is an approximation — accurate near current tick but diverges for large swaps
- Lazy vault fetch for Raydium adds ~10-50ms latency per swap event — acceptable for next-slot arb
