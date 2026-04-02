# Next Session TODO

## Critical (must fix first)

### 1. Mint cache race condition — THE BLOCKER
The async `fetch_mint_program()` is spawned in the Geyser stream but hasn't completed by the time the router tries to build a bundle. Every opportunity fails with "Mint program not cached for X".

**Fix:** In `src/mempool/stream.rs` `process_update()`, await the mint fetch BEFORE calling `tx_sender.try_send(PoolStateChange)`. Change from fire-and-forget spawn to awaited fetch:

```rust
// Current (broken — race condition):
tokio::spawn(async move { fetch_mint_program(...).await; });
// ... immediately sends PoolStateChange

// Fix:
for mint in [pool_mints.0, pool_mints.1] {
    if self.state_cache.get_mint_program(&mint).is_none() {
        let _ = fetch_mint_program(&self.http_client, &self.config.rpc_url, &self.state_cache, &mint).await;
    }
}
// ... then send PoolStateChange
```

**Problem:** `process_update()` is currently `fn` (sync), not `async fn`. The Geyser event loop calls it synchronously. Need to either:
- Make it async (change signature, await in the event loop)
- Or use `tokio::task::block_in_place()` + `Handle::block_on()` for the mint fetch

**Estimated fix: 5-10 min.**

## After the blocker

### 2. Duplicate mint cache fetches
Multiple Geyser events for the same pool spawn redundant `fetch_mint_program` calls. Add a `DashSet<Pubkey>` of "pending fetches" to deduplicate.

### 3. Add compute budget IX
Bundles need a `SetComputeUnitLimit` and `SetComputeUnitPrice` instruction for priority in Jito auctions. Without it, our bundles have lowest priority.

```rust
// Add before swap instructions:
ComputeBudgetInstruction::set_compute_unit_limit(400_000)
ComputeBudgetInstruction::set_compute_unit_price(1_000) // micro-lamports
```

### 4. Verify simulation passes
After fixing #1, capture a bundle tx and run `simulateTransaction` to confirm it passes before doing a live run.

### 5. Live run
With simulation passing, run with DRY_RUN=false and verify bundles land on-chain. Check balance change.

## Reference
- Searcher wallet: `149xtHKerf2MgJVQ2CZB34bUALs8GaZjZWmQnC9si9yh` (0.75 SOL)
- Jito: Frankfurt endpoint, 1 TPS unauth
- Astralane: FRA IP endpoint, 40 TPS, revert_protect=true
- 66 tests passing, 8 DEXes, all swap IX builders complete
