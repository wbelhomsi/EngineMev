# CLMM Fix + Real-Time LST Rates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix CLMM SqrtPriceLimitOverflow and fetch real-time LST/SOL rates from on-chain stake pool accounts (startup RPC + Geyser live updates).

**Architecture:** One-line CLMM fix (sqrt_price_limit=0). LST rates fetched at startup via 2 RPC calls, then live-updated via Geyser subscription on 3 stake pool accounts.

**Tech Stack:** Rust, solana-sdk, reqwest, Yellowstone gRPC

---

### Task 1: Fix CLMM sqrt_price_limit

**Files:**
- Modify: `src/executor/bundle.rs:662-671`
- Modify: `tests/e2e_surfpool/dex_swaps.rs` (remove #[ignore] on CLMM test)

- [ ] **Step 1: Set sqrt_price_limit = 0 in Raydium CLMM builder**

In `src/executor/bundle.rs`, replace the CLMM sqrt_price_limit block (~line 662-671) with:
```rust
// Pass 0 — on-chain program substitutes correct MIN+1/MAX-1
let sqrt_price_limit: u128 = 0u128;
```

- [ ] **Step 2: Remove #[ignore] from CLMM E2E test**

In `tests/e2e_surfpool/dex_swaps.rs`, remove the `#[ignore = "..."]` attribute from `test_raydium_clmm_swap`.

- [ ] **Step 3: Run Surfpool E2E test**

```bash
RPC_URL=$RPC_URL cargo test --features e2e_surfpool --test e2e_surfpool test_raydium_clmm_swap -- --nocapture
```
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/executor/bundle.rs tests/e2e_surfpool/dex_swaps.rs
git commit -m "fix: CLMM sqrt_price_limit=0 — let on-chain program use correct bounds

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin main
```

---

### Task 2: Fetch LST rates at startup

**Files:**
- Modify: `src/main.rs` (add `fetch_lst_rates()`, update fallback rates, call after bootstrap)

- [ ] **Step 1: Update hardcoded fallback rates**

In `bootstrap_sanctum_pools()` (~line 562-568), update the rates:
```rust
"jitoSOL" => 1.271,
"mSOL" => 1.371,
"bSOL" => 1.286,
```

- [ ] **Step 2: Add `fetch_lst_rates()` function**

New async function in main.rs that:
1. Sends batch JSON-RPC: `getMultipleAccounts` for Jito+Blaze pools (dataSlice offset=258, length=16) and `getAccountInfo` for Marinade (dataSlice offset=512, length=8)
2. Parses: jitoSOL/bSOL rate = total_lamports(u64 LE) / pool_token_supply(u64 LE), mSOL rate = msol_price(u64 LE) / 4294967296
3. For each LST, derives the virtual pool address and updates reserves in state_cache

Account addresses from config.rs statics: `JITO_STAKE_POOL`, `BLAZE_STAKE_POOL`, `MARINADE_STATE`.

- [ ] **Step 3: Call fetch_lst_rates at startup**

After `bootstrap_sanctum_pools()` and `bootstrap_lst_indices()`, add:
```rust
if let Err(e) = fetch_lst_rates(&http_client, &config.rpc_url, &state_cache).await {
    warn!("Failed to fetch LST rates: {} — using fallback rates", e);
}
```

- [ ] **Step 4: Verify rates logged**

Run the engine briefly and check for rate logs:
```bash
MIN_PROFIT_LAMPORTS=1000 timeout 15 cargo run --release 2>&1 | grep "LST rate\|rate="
```

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: fetch real-time LST rates from stake pool accounts at startup

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin main
```

---

### Task 3: Add stake pool accounts to Geyser subscription

**Files:**
- Modify: `src/mempool/stream.rs` (subscription + process_update handler)

- [ ] **Step 1: Add stake pool accounts to Geyser subscription**

In `start()` (~line 112-124), after the DEX program owner filters, add individual account filters for the 3 stake pool accounts:

```rust
// Subscribe to stake pool state accounts for real-time LST rate updates
let stake_pool_accounts = vec![
    crate::config::jito_stake_pool().to_string(),
    crate::config::blaze_stake_pool().to_string(),
    crate::config::marinade_state().to_string(),
];
accounts_filter.insert(
    "lst_stake_pools".to_string(),
    SubscribeRequestFilterAccounts {
        account: stake_pool_accounts,
        owner: vec![],
        filters: vec![],
        nonempty_txn_signature: None,
    },
);
```

This adds account-level filters (not program-owner filters).

- [ ] **Step 2: Add accessor functions to config.rs**

Add public accessor functions for the stake pool statics:
```rust
pub fn jito_stake_pool() -> Pubkey { *JITO_STAKE_POOL }
pub fn blaze_stake_pool() -> Pubkey { *BLAZE_STAKE_POOL }
pub fn marinade_state() -> Pubkey { *MARINADE_STATE }
```

- [ ] **Step 3: Handle stake pool updates in process_update**

In `process_update()`, before the DEX data-size routing, check if the account is one of the 3 stake pool accounts. If so, parse the rate and update the virtual pool:

```rust
// Check for stake pool account updates (LST rate changes)
let jito_pool = crate::config::jito_stake_pool();
let blaze_pool = crate::config::blaze_stake_pool();
let marinade = crate::config::marinade_state();

if pool_address == jito_pool || pool_address == blaze_pool {
    // SPL Stake Pool: total_lamports at offset 258, pool_token_supply at 266
    if data.len() >= 274 {
        let total_lamports = u64::from_le_bytes(data[258..266].try_into().unwrap_or_default());
        let supply = u64::from_le_bytes(data[266..274].try_into().unwrap_or_default());
        if supply > 0 {
            let rate = total_lamports as f64 / supply as f64;
            let lst_mint = if pool_address == jito_pool {
                crate::config::lst_mints().into_iter().find(|(_, n)| *n == "jitoSOL").map(|(m, _)| m)
            } else {
                crate::config::lst_mints().into_iter().find(|(_, n)| *n == "bSOL").map(|(m, _)| m)
            };
            if let Some(mint) = lst_mint {
                update_sanctum_virtual_pool(&self.state_cache, &mint, rate);
            }
        }
    }
    return;
} else if pool_address == marinade {
    // Marinade: msol_price at offset 512
    if data.len() >= 520 {
        let msol_price = u64::from_le_bytes(data[512..520].try_into().unwrap_or_default());
        let rate = msol_price as f64 / 4_294_967_296.0; // 2^32 denominator
        if rate > 0.5 && rate < 5.0 {
            let mint = crate::config::lst_mints().into_iter().find(|(_, n)| *n == "mSOL").map(|(m, _)| m);
            if let Some(mint) = mint {
                update_sanctum_virtual_pool(&self.state_cache, &mint, rate);
            }
        }
    }
    return;
}
```

- [ ] **Step 4: Add update_sanctum_virtual_pool helper**

```rust
fn update_sanctum_virtual_pool(state_cache: &StateCache, lst_mint: &Pubkey, rate: f64) {
    let (virtual_pool_addr, _) = Pubkey::find_program_address(
        &[b"sanctum-virtual", lst_mint.as_ref()],
        &solana_sdk::system_program::id(),
    );
    if let Some(mut pool) = state_cache.get_any(&virtual_pool_addr) {
        let reserve_a: u64 = 1_000_000_000_000_000;
        pool.token_a_reserve = reserve_a;
        pool.token_b_reserve = (reserve_a as f64 / rate) as u64;
        state_cache.upsert(virtual_pool_addr, pool);
        debug!("Updated LST rate for {}: {:.6}", lst_mint, rate);
    }
}
```

- [ ] **Step 5: Verify compilation + unit tests**

```bash
cargo check && cargo test --test unit
```

- [ ] **Step 6: Commit**

```bash
git add src/mempool/stream.rs src/config.rs
git commit -m "feat: Geyser subscription for stake pool accounts — real-time LST rates

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin main
```

---

### Task 4: Verify with Surfpool E2E + live run

- [ ] **Step 1: Run all Surfpool E2E tests**

```bash
RPC_URL=$RPC_URL cargo test --features e2e_surfpool --test e2e_surfpool -- --test-threads=1 --nocapture
```
Expected: 5+ passed (CLMM now unignored)

- [ ] **Step 2: Run live for 2 minutes**

```bash
MIN_PROFIT_LAMPORTS=1000 SKIP_SIMULATOR=true timeout 120 cargo run --release 2>&1 | grep "LST rate\|OPPORTUNITY\|accepted"
```
Expected: LST rate logs at startup, opportunities with realistic profits (not billions)

- [ ] **Step 3: Commit docs update**

```bash
git add docs/
git commit -m "docs: CLMM fixed, real-time LST rates verified

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin main
```
