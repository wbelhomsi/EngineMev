# Geyser Stream Fix: Parse Pool State Accounts, Not Token Vaults

## Problem

The Geyser subscription in `stream.rs` watches accounts owned by DEX programs (Raydium, Orca, Meteora). These are **pool state accounts** (AmmInfo, Whirlpool, LbPair), not SPL Token vaults. But `process_update` parses bytes 64..72 as an SPL Token balance — completely wrong layout. Result: garbage data, no useful events reach the router.

## Solution

Replace the single SPL Token parser with per-DEX pool state parsers. When a pool state account update arrives via Geyser, identify which DEX owns it, parse the relevant fields from the pool-specific layout, and update the PoolState in the StateCache directly.

## Architecture

### Subscription (unchanged)

The current subscription is correct — watch accounts owned by DEX programs:
- `dex_0`: Raydium AMM v4 (`675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8`)
- `dex_1`: Raydium CLMM (`CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK`)
- `dex_2`: Orca Whirlpool (`whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc`)
- `dex_3`: Meteora DLMM (`LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo`)
- `dex_4`: Sanctum S Controller (`5ocnV1qiCgaQR8Jb8xWnVbApfaygJ8tNoZfgPwsgx9kx`)

The filter key (`dex_0`, `dex_1`, etc.) tells us which program owns the account.

### Event Processing (rewritten)

When a Geyser account update arrives:

1. **Identify DEX** from the subscription filter key in the update
2. **Validate data size** matches expected pool account (752 for Raydium AMM, 653 for Orca, 904 for Meteora, etc.)
3. **Parse per-DEX** — extract the fields we need:
   - **Orca Whirlpool (653 bytes):** `sqrt_price` (offset 65, u128), `tick_current_index` (offset 81, i32), `liquidity` (offset 49, u128), `token_mint_a` (offset 101), `token_mint_b` (offset 181), `token_vault_a` (offset 133), `token_vault_b` (offset 213)
   - **Meteora DLMM (904 bytes):** `active_id` (offset 76, i32), `bin_step` (offset 80, u16), `token_x_mint` (offset 88), `token_y_mint` (offset 120), `reserve_x` (offset 152), `reserve_y` (offset 184)
   - **Raydium AMM v4 (752 bytes):** `coin_vault` (offset 336), `pc_vault` (offset 368), `coin_vault_mint` (offset 400), `pc_vault_mint` (offset 432). Also: `need_take_pnl_coin` and `need_take_pnl_pc` from StateData for reserve calculation.
4. **Update StateCache** — upsert the PoolState with fresh data, register vaults (if first time seeing this pool)
5. **Notify router** — send a signal through the existing crossbeam channel that a pool changed

### PoolStateChange Redesign

Current:
```rust
pub struct PoolStateChange {
    pub vault_address: Pubkey,  // wrong — this is a pool address
    pub new_balance: u64,       // wrong — meaningless for pool state
    pub slot: u64,
}
```

New:
```rust
pub struct PoolStateChange {
    pub pool_address: Pubkey,
    pub slot: u64,
}
```

The router no longer needs balance data in the event — it reads the full PoolState from the cache (which was just updated by the stream). The event is just a "pool X changed at slot Y" notification.

### Router Changes

Current router flow:
1. Receive PoolStateChange with vault_address + balance
2. Look up vault→pool in index
3. Update vault balance in cache
4. Build trigger from pool state
5. Find routes + simulate

New router flow (simpler):
1. Receive PoolStateChange with pool_address
2. Look up pool in cache (already updated by stream)
3. Build trigger from pool state
4. Find routes + simulate

Steps 2-3 of the old flow (vault lookup + balance update) are eliminated — the stream handles the cache update directly.

### Per-DEX Reserve Calculation

**Orca Whirlpool:** No reserves needed in the traditional sense. The router uses `sqrt_price_x64`, `tick_current_index`, and `liquidity` for CLMM math. These come directly from the pool state. For the current constant-product approximation, we can derive synthetic reserves from sqrt_price: `reserve_a = liquidity / sqrt_price`, `reserve_b = liquidity * sqrt_price`. This gives the router something to work with until we implement proper tick-crossing math.

**Meteora DLMM:** The `active_id` and `bin_step` define the current price. Similar to Orca, we can derive synthetic reserves for the constant-product approximation: `price = (1 + bin_step/10000)^(active_id - 2^23)`. For now, set reserves to produce this price ratio with large synthetic values (same pattern as Sanctum virtual pools).

**Raydium AMM v4:** Reserves = vault_balance - need_take_pnl. The vault_balance comes from the startup bootstrap (background `getMultipleAccounts`). The `need_take_pnl_coin` and `need_take_pnl_pc` come from the AmmInfo state update via Geyser. On each Geyser update, recalculate: `effective_reserve = cached_vault_balance - need_take_pnl`. Until vault balances are bootstrapped, Raydium pools have zero reserves (router skips them).

### Raydium need_take_pnl Offsets

From the AmmInfo StateData struct (starts after Fees at offset 192):
- StateData is at offset 192, and contains multiple u128 and u64 fields
- `need_take_pnl_coin`: offset 224, u64 (8 bytes)
- `need_take_pnl_pc`: offset 232, u64 (8 bytes)

These offsets need verification against the actual Raydium source before implementation.

### Bootstrap Changes

- **Keep:** Raydium vault balance fetch via `getMultipleAccounts` (background, non-blocking). This gives us initial vault_balance for the reserve calculation.
- **Remove:** Orca and Meteora pool bootstrapping via `getProgramAccounts`. Geyser gives us everything we need for these DEXes — pools self-register on first update.
- **Keep:** Raydium `getProgramAccounts` — needed to discover vault addresses for balance fetch, and to populate the initial pool index so we know which pools exist.

This cuts bootstrap from 3 DEX fetches to 1 (Raydium only). Orca and Meteora pools register automatically as Geyser events arrive.

### Vault Balance Storage

Add a new field to PoolState or a separate index for Raydium vault balances:

```rust
// In StateCache, add a vault balance store for Raydium reserve calculation
vault_balances: Arc<DashMap<Pubkey, u64>>,  // vault_address → balance
```

On bootstrap: populate vault_balances from getMultipleAccounts.
On Geyser AmmInfo update: read vault_balances for this pool's vaults, subtract need_take_pnl, set as reserves.

## Files Changed

| File | Action | What |
|------|--------|------|
| `src/mempool/stream.rs` | Rewrite | Per-DEX pool state parsing, direct cache update, simplified PoolStateChange |
| `src/mempool/mod.rs` | Modify | Update PoolStateChange export |
| `src/state/cache.rs` | Modify | Add vault_balances DashMap for Raydium |
| `src/main.rs` | Modify | Simplify router loop (no vault lookup), pass cache to stream |
| `src/state/bootstrap.rs` | Modify | Remove Orca/Meteora bootstrap, keep Raydium only + vault balance fetch |
| `tests/unit/stream_parsing.rs` | Create | Unit tests for per-DEX Geyser event parsing |
| `tests/e2e/lst_pipeline.rs` | Modify | Update to use new PoolStateChange |

## Testing

- **Unit tests:** Feed raw account data bytes (same test helpers from bootstrap.rs) into the stream parser, verify PoolState is correctly extracted for each DEX
- **E2E:** Update existing e2e tests for the new PoolStateChange format
- **Manual:** Run with DRY_RUN=true against Helius LaserStream, verify Orca/Meteora pools appear within seconds and Raydium joins after bootstrap

## Risk Notes

- Raydium `need_take_pnl` offsets (224/232) are estimated from struct layout — must verify against source before implementing
- Orca/Meteora synthetic reserve derivation from CLMM parameters is an approximation — good enough for route discovery but not for precise profit simulation
- Removing Orca/Meteora from bootstrap means we only see pools that have activity during our session — cold pools are invisible. This is fine since we can only arb active pools anyway.
