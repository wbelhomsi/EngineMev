# Phase 1 Completion: Pool Bootstrapping, Blockhash Cache, Geyser Reconnect

## Goal

Make the MEV engine runnable against a real Solana cluster (dry-run mode). Three features are needed:

1. **Pool state bootstrapping** — populate StateCache + vault→pool index at startup via `getProgramAccounts`
2. **Recent blockhash cache** — background task fetching `getLatestBlockhash` every ~2s
3. **Geyser stream reconnect** — retry loop with exponential backoff on disconnect

## Prerequisites

- Helius or Triton RPC provider (supports `getProgramAccounts` with filters)
- Same provider for Geyser gRPC
- Existing codebase with Phase 2 LST arb complete

---

## Feature 1: Pool State Bootstrapping

### Architecture

New module: `src/state/bootstrap.rs`

At startup, before the Geyser stream starts, call `getProgramAccounts` for each DEX program. Parse pool accounts, extract vault pubkeys and mints, populate `StateCache.upsert()` and `StateCache.register_vault()`.

### RPC Calls

One `getProgramAccounts` call per DEX, with `dataSize` filter and `dataSlice` to minimize bandwidth:

| DEX | Program ID | dataSize filter | dataSlice (offset, length) |
|-----|-----------|----------------|---------------------------|
| Raydium AMM v4 | `675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8` | 752 | (240, 128) — covers vaults + mints |
| Orca Whirlpool | `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc` | 653 | (49, 196) — covers liquidity through vault_b |
| Meteora DLMM | `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo` | 902 | (74, 140) — covers active_id through reserve_y |

For Raydium, also filter `memcmp` at offset 0 with value `06 00 00 00 00 00 00 00` (status = 6, active pools only).

### Account Layouts — Vault + Mint Offsets

**Raydium AMM v4** (no Anchor discriminator):
- `coin_vault` (token A vault): offset 240, 32 bytes
- `pc_vault` (token B vault): offset 272, 32 bytes
- `coin_vault_mint` (token A mint): offset 304, 32 bytes
- `pc_vault_mint` (token B mint): offset 336, 32 bytes

**Orca Whirlpool** (8-byte Anchor discriminator):
- `liquidity`: offset 49, 16 bytes (u128)
- `sqrt_price`: offset 65, 16 bytes (u128)
- `tick_current_index`: offset 81, 4 bytes (i32)
- `token_mint_a`: offset 101, 32 bytes
- `token_vault_a`: offset 133, 32 bytes
- `token_mint_b`: offset 181, 32 bytes
- `token_vault_b`: offset 213, 32 bytes

**Meteora DLMM** (8-byte Anchor discriminator):
- `active_id`: offset 74, 4 bytes (i32)
- `bin_step`: offset 78, 2 bytes (u16)
- `token_x_mint`: offset 86, 32 bytes
- `token_y_mint`: offset 118, 32 bytes
- `reserve_x` (vault X): offset 150, 32 bytes
- `reserve_y` (vault Y): offset 182, 32 bytes

### Vault Balance Fetch

After discovering pools, we need initial vault balances. Two options:

**Option A (chosen): Batch `getMultipleAccounts` on vault pubkeys.**
- After parsing all pools, collect unique vault pubkeys
- Call `getMultipleAccounts` in batches of 100 (RPC limit)
- Parse SPL Token balance at bytes 64..72 (same as Geyser does)
- Populate reserves in the cached PoolState

### Flow

```
startup:
  for each DEX program:
    getProgramAccounts(program, filters) → pool accounts
    parse vault pubkeys + mints from account data
    StateCache.upsert(pool_address, PoolState)
    StateCache.register_vault(vault_a, pool_address, true)
    StateCache.register_vault(vault_b, pool_address, false)

  collect all vault pubkeys
  getMultipleAccounts(vaults, batches of 100) → vault balances
  for each vault:
    StateCache.update_vault_balance(vault, balance, slot)

  log: "Bootstrapped {n} pools, {m} vaults"
```

### Error Handling

- If `getProgramAccounts` fails for one DEX, log error and continue with others
- If `getMultipleAccounts` fails for a batch, retry once, then skip (Geyser will fill in)
- If zero pools bootstrapped, log critical warning but don't crash (Geyser may still work for known pools)

### Config

No new env vars needed. Uses existing `RPC_URL`. Bootstrapping runs once at startup, blocking before Geyser starts.

---

## Feature 2: Recent Blockhash Cache

### Architecture

New module: `src/state/blockhash.rs`

Shared state: `Arc<RwLock<BlockhashInfo>>` where:

```rust
pub struct BlockhashInfo {
    pub blockhash: Hash,
    pub last_valid_block_height: u64,
    pub fetched_at: Instant,
}
```

### Background Task

Spawned in `main.rs` alongside the Geyser stream:

```
loop:
  getLatestBlockhash(commitment: "confirmed")
  update Arc<RwLock<BlockhashInfo>>
  sleep 2s
  check shutdown signal
```

### Consumer API

```rust
impl BlockhashCache {
    pub fn get(&self) -> Option<Hash>
    // Returns None if blockhash is older than 5s (stale guard)
}
```

The bundle builder calls `blockhash_cache.get()` instead of using `Hash::default()`. If `None`, the opportunity is skipped (logged as "stale blockhash").

### Error Handling

- If RPC call fails, keep the old blockhash (it's valid for ~60s / 150 blocks)
- Log warning on failure, don't crash
- After 3 consecutive failures, log error (RPC may be down)

---

## Feature 3: Geyser Stream Reconnect

### Architecture

No new module. Modify `main.rs` to wrap the Geyser stream in a retry loop.

### Retry Logic

```
let mut backoff = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

loop:
  match geyser_stream.start(change_tx.clone(), shutdown_rx.clone()).await:
    Ok(()) => info!("Geyser stream ended cleanly")
    Err(e) => error!("Geyser stream error: {}", e)

  if shutdown:
    break

  warn!("Reconnecting in {:?}...", backoff)
  sleep(backoff)
  backoff = min(backoff * 2, MAX_BACKOFF)

  on successful reconnect:
    backoff = Duration::from_secs(1)  // reset
```

### Channel Handling

The `crossbeam_channel::Sender` is `Clone`. The reconnect loop clones it for each new `start()` call. The receiver in the router thread stays the same — it just sees a gap in events during reconnect (acceptable, stale events would be useless anyway).

### GeyserStream Changes

`GeyserStream::start()` currently returns `Result<()>`. No signature change needed. It returns `Ok(())` on clean shutdown, `Err` on disconnect. The caller handles retry.

---

## Files Changed

| File | Action | What |
|------|--------|------|
| `src/state/bootstrap.rs` | Create | Pool bootstrapping: `getProgramAccounts` + vault balance fetch |
| `src/state/blockhash.rs` | Create | Blockhash cache: background fetch + shared state |
| `src/state/mod.rs` | Modify | Export new modules |
| `src/main.rs` | Modify | Call bootstrap at startup, spawn blockhash task, Geyser reconnect loop, pass blockhash to bundle builder |
| `src/executor/bundle.rs` | Modify | Accept `BlockhashCache` instead of `Hash::default()` |
| `tests/unit/bootstrap.rs` | Create | Unit tests for account parsing |
| `tests/unit/blockhash.rs` | Create | Unit tests for cache staleness logic |
| `tests/unit/mod.rs` | Modify | Add new test modules |

## Testing Strategy

- **Unit tests:** Parse known account data bytes into PoolState (mock RPC responses)
- **Unit tests:** Blockhash cache staleness, get() returns None when stale
- **E2E:** Bootstrap with Surfpool (if available) or mock RPC responses
- **Manual:** Run with `DRY_RUN=true` against Helius mainnet, verify pools load and opportunities log

## Risk Notes

- `getProgramAccounts` can return 100K+ Raydium pools — we filter by status=6 (active) but it may still be large. Consider limiting to pools with >$1K TVL in a follow-up.
- Meteora DLMM account size (902 bytes) should be verified against a live mainnet account before hardcoding — the IDL has historically drifted.
- Blockhash cache introduces a shared `RwLock` on the hot path. This is acceptable — `RwLock::read()` is non-blocking when no writer holds it, and writes happen only every 2s.
