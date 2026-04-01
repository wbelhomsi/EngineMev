# Phase 1 Completion: Pool Bootstrapping, Blockhash Cache, Geyser Reconnect — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the MEV engine runnable against a real Solana cluster in dry-run mode by adding pool state bootstrapping, blockhash caching, and Geyser reconnect.

**Architecture:** Three bolt-on features to the existing pipeline. `bootstrap.rs` populates the StateCache at startup via `getProgramAccounts`. `blockhash.rs` runs a 2s background loop fetching `getLatestBlockhash`. Geyser reconnect is a retry loop in `main.rs` wrapping the existing stream.

**Tech Stack:** Rust, solana-sdk 2.2, reqwest (RPC JSON-RPC calls), existing crate dependencies.

**Prerequisite:** Run `cargo check` to verify the project compiles before starting.

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `src/state/bootstrap.rs` | Create | DEX pool account parsing + `getProgramAccounts` + `getMultipleAccounts` vault balance fetch |
| `src/state/blockhash.rs` | Create | `BlockhashCache` struct with `Arc<RwLock>`, background refresh task, staleness guard |
| `src/state/mod.rs` | Modify | Export `bootstrap` and `blockhash` modules |
| `src/main.rs` | Modify | Call bootstrap at startup, spawn blockhash task, Geyser reconnect loop, pass blockhash cache to router |
| `src/executor/bundle.rs` | Modify | Remove `Hash::default()` placeholder, accept blockhash from cache |
| `tests/unit/bootstrap.rs` | Create | Unit tests for Raydium/Orca/Meteora account data parsing |
| `tests/unit/blockhash.rs` | Create | Unit tests for cache staleness logic |
| `tests/unit/mod.rs` | Modify | Add `mod bootstrap; mod blockhash;` |

---

### Task 1: Blockhash cache — struct + staleness logic

**Files:**
- Create: `src/state/blockhash.rs`
- Create: `tests/unit/blockhash.rs`
- Modify: `tests/unit/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/unit/blockhash.rs`:

```rust
use std::time::{Duration, Instant};
use solana_sdk::hash::Hash;

use solana_mev_bot::state::blockhash::{BlockhashCache, BlockhashInfo};

#[test]
fn test_blockhash_cache_returns_fresh() {
    let cache = BlockhashCache::new();
    let hash = Hash::new_unique();
    cache.update(BlockhashInfo {
        blockhash: hash,
        last_valid_block_height: 1000,
        fetched_at: Instant::now(),
    });
    assert_eq!(cache.get(), Some(hash));
}

#[test]
fn test_blockhash_cache_returns_none_when_empty() {
    let cache = BlockhashCache::new();
    assert_eq!(cache.get(), None);
}

#[test]
fn test_blockhash_cache_returns_none_when_stale() {
    let cache = BlockhashCache::new();
    let hash = Hash::new_unique();
    // Simulate a blockhash fetched 10s ago (stale threshold is 5s)
    cache.update(BlockhashInfo {
        blockhash: hash,
        last_valid_block_height: 1000,
        fetched_at: Instant::now() - Duration::from_secs(10),
    });
    assert_eq!(cache.get(), None, "Stale blockhash should return None");
}

#[test]
fn test_blockhash_cache_clone_shares_state() {
    let cache1 = BlockhashCache::new();
    let cache2 = cache1.clone();
    let hash = Hash::new_unique();
    cache1.update(BlockhashInfo {
        blockhash: hash,
        last_valid_block_height: 1000,
        fetched_at: Instant::now(),
    });
    assert_eq!(cache2.get(), Some(hash), "Cloned cache should see update");
}
```

Add to `tests/unit/mod.rs`:

```rust
mod blockhash;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test unit blockhash -- --nocapture`

Expected: Compilation error — `solana_mev_bot::state::blockhash` does not exist.

- [ ] **Step 3: Implement BlockhashCache**

Create `src/state/blockhash.rs`:

```rust
use solana_sdk::hash::Hash;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// Maximum age of a cached blockhash before it's considered stale.
/// Blockhashes are valid for ~60s (150 blocks), but we're aggressive —
/// a 5s-old blockhash in a latency game is already risky.
const STALE_THRESHOLD: Duration = Duration::from_secs(5);

/// Information about a cached blockhash.
#[derive(Debug, Clone)]
pub struct BlockhashInfo {
    pub blockhash: Hash,
    pub last_valid_block_height: u64,
    pub fetched_at: Instant,
}

/// Thread-safe blockhash cache shared between the background fetcher
/// and the router thread. Uses `RwLock` — writes happen every 2s,
/// reads happen on the hot path but `read()` is non-blocking when
/// no writer holds the lock.
#[derive(Clone)]
pub struct BlockhashCache {
    inner: Arc<RwLock<Option<BlockhashInfo>>>,
}

impl BlockhashCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
        }
    }

    /// Update the cached blockhash. Called by the background fetcher.
    pub fn update(&self, info: BlockhashInfo) {
        let mut guard = self.inner.write().unwrap();
        *guard = Some(info);
    }

    /// Get the cached blockhash if it's fresh enough.
    /// Returns None if empty or older than STALE_THRESHOLD.
    pub fn get(&self) -> Option<Hash> {
        let guard = self.inner.read().unwrap();
        guard.as_ref().and_then(|info| {
            if info.fetched_at.elapsed() < STALE_THRESHOLD {
                Some(info.blockhash)
            } else {
                None
            }
        })
    }
}
```

- [ ] **Step 4: Export the module**

Update `src/state/mod.rs`:

```rust
pub mod cache;
pub mod blockhash;

pub use cache::StateCache;
pub use blockhash::BlockhashCache;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test unit blockhash -- --nocapture`

Expected: All 4 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add src/state/blockhash.rs src/state/mod.rs tests/unit/blockhash.rs tests/unit/mod.rs
git commit -m "feat: add BlockhashCache with staleness guard and shared state"
```

---

### Task 2: Blockhash background fetcher + main.rs integration

**Files:**
- Modify: `src/state/blockhash.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Add the async fetch function to blockhash.rs**

Append to `src/state/blockhash.rs`:

```rust
use tracing::{info, warn, error};

/// Fetch the latest blockhash from RPC and update the cache.
/// Returns Ok(()) on success, Err on RPC failure.
pub async fn fetch_and_update(
    client: &reqwest::Client,
    rpc_url: &str,
    cache: &BlockhashCache,
) -> anyhow::Result<()> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getLatestBlockhash",
        "params": [{ "commitment": "confirmed" }]
    });

    let resp = client
        .post(rpc_url)
        .json(&payload)
        .timeout(Duration::from_secs(5))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let value = resp
        .get("result")
        .and_then(|r| r.get("value"))
        .ok_or_else(|| anyhow::anyhow!("Missing result.value in getLatestBlockhash response"))?;

    let blockhash_str = value
        .get("blockhash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing blockhash field"))?;

    let last_valid_block_height = value
        .get("lastValidBlockHeight")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow::anyhow!("Missing lastValidBlockHeight field"))?;

    let blockhash: Hash = blockhash_str.parse()
        .map_err(|_| anyhow::anyhow!("Invalid blockhash: {}", blockhash_str))?;

    cache.update(BlockhashInfo {
        blockhash,
        last_valid_block_height,
        fetched_at: Instant::now(),
    });

    Ok(())
}

/// Spawn the background blockhash refresh loop.
/// Fetches every 2s, logs warnings on failure, resets error count on success.
pub async fn run_blockhash_loop(
    client: reqwest::Client,
    rpc_url: String,
    cache: BlockhashCache,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut consecutive_failures: u32 = 0;

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() { break; }
            }
            _ = tokio::time::sleep(Duration::from_secs(2)) => {
                match fetch_and_update(&client, &rpc_url, &cache).await {
                    Ok(()) => {
                        if consecutive_failures > 0 {
                            info!("Blockhash fetch recovered after {} failures", consecutive_failures);
                        }
                        consecutive_failures = 0;
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        if consecutive_failures >= 3 {
                            error!("Blockhash fetch failed {} times: {}", consecutive_failures, e);
                        } else {
                            warn!("Blockhash fetch failed ({}x): {}", consecutive_failures, e);
                        }
                    }
                }
            }
        }
    }

    info!("Blockhash refresh loop exited");
}
```

Add the required imports at the top of `src/state/blockhash.rs` (merge with existing):

```rust
use solana_sdk::hash::Hash;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::{info, warn, error};
```

- [ ] **Step 2: Integrate into main.rs — spawn blockhash task, pass cache to router**

In `src/main.rs`, after the state cache setup and before the shutdown signal, add:

```rust
    // Initialize blockhash cache and do first fetch
    let blockhash_cache = state::BlockhashCache::new();
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    {
        // Fetch first blockhash synchronously before starting pipeline
        if let Err(e) = state::blockhash::fetch_and_update(&http_client, &config.rpc_url, &blockhash_cache).await {
            warn!("Initial blockhash fetch failed (will retry in background): {}", e);
        } else {
            info!("Initial blockhash fetched");
        }
    }
```

After the Ctrl+C handler, spawn the blockhash loop as a new task:

```rust
    // Task: Blockhash refresh (async, I/O bound, 2s interval)
    let blockhash_handle = {
        let client = http_client.clone();
        let rpc_url = config.rpc_url.clone();
        let cache = blockhash_cache.clone();
        let shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            state::blockhash::run_blockhash_loop(client, rpc_url, cache, shutdown_rx).await;
        })
    };
```

Replace the `Hash::default()` placeholder in the router loop:

Replace:
```rust
                        // Build and submit bundle
                        // TODO: get recent blockhash from RPC (cache with ~2s TTL)
                        let blockhash = solana_sdk::hash::Hash::default(); // placeholder
```

With:
```rust
                        // Get recent blockhash from cache
                        let blockhash = match blockhash_cache.get() {
                            Some(h) => h,
                            None => {
                                warn!("Stale or missing blockhash, skipping opportunity");
                                continue;
                            }
                        };
```

To make `blockhash_cache` available inside the `spawn_blocking` closure, clone it alongside other variables:

In the router_handle block, add `let blockhash_cache = blockhash_cache.clone();` alongside the existing `let config = config.clone();`.

Add `blockhash_handle` to the final `tokio::try_join!`:

Replace:
```rust
    let _ = tokio::try_join!(stream_handle, cache_handle);
```

With:
```rust
    let _ = tokio::try_join!(stream_handle, cache_handle, blockhash_handle);
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check`

Expected: Compiles cleanly.

- [ ] **Step 4: Commit**

```bash
git add src/state/blockhash.rs src/main.rs
git commit -m "feat: blockhash background fetcher + main.rs integration"
```

---

### Task 3: Pool bootstrapping — account data parsers

**Files:**
- Create: `src/state/bootstrap.rs`
- Create: `tests/unit/bootstrap.rs`
- Modify: `tests/unit/mod.rs`

- [ ] **Step 1: Write the failing tests**

Create `tests/unit/bootstrap.rs`:

```rust
use solana_sdk::pubkey::Pubkey;

use solana_mev_bot::router::pool::DexType;
use solana_mev_bot::state::bootstrap::{
    parse_raydium_amm_pool,
    parse_orca_whirlpool_pool,
    parse_meteora_dlmm_pool,
};

/// Helper: build a fake Raydium AMM account data buffer (752 bytes).
/// Sets status=6 at offset 0, vaults at 336/368, mints at 400/432.
fn make_raydium_data(
    coin_vault: &Pubkey,
    pc_vault: &Pubkey,
    coin_mint: &Pubkey,
    pc_mint: &Pubkey,
) -> Vec<u8> {
    let mut data = vec![0u8; 752];
    // status = 6 (active) at offset 0, u64 LE
    data[0..8].copy_from_slice(&6u64.to_le_bytes());
    data[336..368].copy_from_slice(coin_vault.as_ref());
    data[368..400].copy_from_slice(pc_vault.as_ref());
    data[400..432].copy_from_slice(coin_mint.as_ref());
    data[432..464].copy_from_slice(pc_mint.as_ref());
    data
}

/// Helper: build a fake Orca Whirlpool account data buffer (653 bytes).
fn make_whirlpool_data(
    mint_a: &Pubkey,
    vault_a: &Pubkey,
    mint_b: &Pubkey,
    vault_b: &Pubkey,
    sqrt_price: u128,
    tick: i32,
    liquidity: u128,
) -> Vec<u8> {
    let mut data = vec![0u8; 653];
    // 8-byte Anchor discriminator at offset 0 (we don't validate it)
    data[49..65].copy_from_slice(&liquidity.to_le_bytes());
    data[65..81].copy_from_slice(&sqrt_price.to_le_bytes());
    data[81..85].copy_from_slice(&tick.to_le_bytes());
    data[101..133].copy_from_slice(mint_a.as_ref());
    data[133..165].copy_from_slice(vault_a.as_ref());
    data[181..213].copy_from_slice(mint_b.as_ref());
    data[213..245].copy_from_slice(vault_b.as_ref());
    data
}

/// Helper: build a fake Meteora DLMM account data buffer (902 bytes).
fn make_meteora_data(
    mint_x: &Pubkey,
    mint_y: &Pubkey,
    reserve_x: &Pubkey,
    reserve_y: &Pubkey,
    active_id: i32,
    bin_step: u16,
) -> Vec<u8> {
    let mut data = vec![0u8; 920];
    // 8-byte Anchor discriminator at offset 0
    data[76..80].copy_from_slice(&active_id.to_le_bytes());
    data[80..82].copy_from_slice(&bin_step.to_le_bytes());
    data[88..120].copy_from_slice(mint_x.as_ref());
    data[120..152].copy_from_slice(mint_y.as_ref());
    data[152..184].copy_from_slice(reserve_x.as_ref());
    data[184..216].copy_from_slice(reserve_y.as_ref());
    data
}

#[test]
fn test_parse_raydium_amm_pool() {
    let pool_addr = Pubkey::new_unique();
    let coin_vault = Pubkey::new_unique();
    let pc_vault = Pubkey::new_unique();
    let coin_mint = Pubkey::new_unique();
    let pc_mint = Pubkey::new_unique();

    let data = make_raydium_data(&coin_vault, &pc_vault, &coin_mint, &pc_mint);
    let result = parse_raydium_amm_pool(&pool_addr, &data);
    assert!(result.is_some(), "Should parse valid Raydium data");

    let (pool, vault_a, vault_b) = result.unwrap();
    assert_eq!(pool.address, pool_addr);
    assert_eq!(pool.dex_type, DexType::RaydiumAmm);
    assert_eq!(pool.token_a_mint, coin_mint);
    assert_eq!(pool.token_b_mint, pc_mint);
    assert_eq!(vault_a, coin_vault);
    assert_eq!(vault_b, pc_vault);
}

#[test]
fn test_parse_raydium_rejects_short_data() {
    let pool_addr = Pubkey::new_unique();
    let data = vec![0u8; 100]; // too short
    assert!(parse_raydium_amm_pool(&pool_addr, &data).is_none());
}

#[test]
fn test_parse_orca_whirlpool() {
    let pool_addr = Pubkey::new_unique();
    let mint_a = Pubkey::new_unique();
    let vault_a = Pubkey::new_unique();
    let mint_b = Pubkey::new_unique();
    let vault_b = Pubkey::new_unique();

    let data = make_whirlpool_data(&mint_a, &vault_a, &mint_b, &vault_b, 1_000_000, -100, 500_000);
    let result = parse_orca_whirlpool_pool(&pool_addr, &data);
    assert!(result.is_some(), "Should parse valid Whirlpool data");

    let (pool, va, vb) = result.unwrap();
    assert_eq!(pool.address, pool_addr);
    assert_eq!(pool.dex_type, DexType::OrcaWhirlpool);
    assert_eq!(pool.token_a_mint, mint_a);
    assert_eq!(pool.token_b_mint, mint_b);
    assert_eq!(pool.sqrt_price_x64, Some(1_000_000));
    assert_eq!(pool.current_tick, Some(-100));
    assert_eq!(pool.liquidity, Some(500_000));
    assert_eq!(va, vault_a);
    assert_eq!(vb, vault_b);
}

#[test]
fn test_parse_meteora_dlmm() {
    let pool_addr = Pubkey::new_unique();
    let mint_x = Pubkey::new_unique();
    let mint_y = Pubkey::new_unique();
    let reserve_x = Pubkey::new_unique();
    let reserve_y = Pubkey::new_unique();

    let data = make_meteora_data(&mint_x, &mint_y, &reserve_x, &reserve_y, 42, 10);
    let result = parse_meteora_dlmm_pool(&pool_addr, &data);
    assert!(result.is_some(), "Should parse valid Meteora data");

    let (pool, vx, vy) = result.unwrap();
    assert_eq!(pool.address, pool_addr);
    assert_eq!(pool.dex_type, DexType::MeteoraDlmm);
    assert_eq!(pool.token_a_mint, mint_x);
    assert_eq!(pool.token_b_mint, mint_y);
    assert_eq!(vx, reserve_x);
    assert_eq!(vy, reserve_y);
}
```

Add to `tests/unit/mod.rs`:

```rust
mod bootstrap;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test unit bootstrap -- --nocapture`

Expected: Compilation error — `solana_mev_bot::state::bootstrap` does not exist.

- [ ] **Step 3: Implement the parsers**

Create `src/state/bootstrap.rs`:

```rust
use solana_sdk::pubkey::Pubkey;
use tracing::{info, warn, error, debug};

use crate::router::pool::{DexType, PoolState};
use crate::state::StateCache;

// ── Account data parsers ──────────────────────────────────────────

/// Parse a Raydium AMM v4 pool account.
/// Returns (PoolState, vault_a_pubkey, vault_b_pubkey) or None if data is invalid.
///
/// Layout (no Anchor discriminator, 752 bytes):
///   offset 336: coin_vault (Pubkey, 32B)
///   offset 368: pc_vault (Pubkey, 32B)
///   offset 400: coin_vault_mint (Pubkey, 32B)
///   offset 432: pc_vault_mint (Pubkey, 32B)
pub fn parse_raydium_amm_pool(
    pool_address: &Pubkey,
    data: &[u8],
) -> Option<(PoolState, Pubkey, Pubkey)> {
    if data.len() < 464 {
        return None;
    }

    let coin_vault = Pubkey::try_from(&data[336..368]).ok()?;
    let pc_vault = Pubkey::try_from(&data[368..400]).ok()?;
    let coin_mint = Pubkey::try_from(&data[400..432]).ok()?;
    let pc_mint = Pubkey::try_from(&data[432..464]).ok()?;

    let pool = PoolState {
        address: *pool_address,
        dex_type: DexType::RaydiumAmm,
        token_a_mint: coin_mint,
        token_b_mint: pc_mint,
        token_a_reserve: 0, // filled by vault balance fetch
        token_b_reserve: 0,
        fee_bps: DexType::RaydiumAmm.base_fee_bps(),
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 0,
    };

    Some((pool, coin_vault, pc_vault))
}

/// Parse an Orca Whirlpool pool account.
/// Returns (PoolState, vault_a_pubkey, vault_b_pubkey) or None if data is invalid.
///
/// Layout (8-byte Anchor discriminator, 653 bytes total):
///   offset 49: liquidity (u128 LE, 16B)
///   offset 65: sqrt_price (u128 LE, 16B)
///   offset 81: tick_current_index (i32 LE, 4B)
///   offset 101: token_mint_a (Pubkey, 32B)
///   offset 133: token_vault_a (Pubkey, 32B)
///   offset 181: token_mint_b (Pubkey, 32B)
///   offset 213: token_vault_b (Pubkey, 32B)
pub fn parse_orca_whirlpool_pool(
    pool_address: &Pubkey,
    data: &[u8],
) -> Option<(PoolState, Pubkey, Pubkey)> {
    if data.len() < 245 {
        return None;
    }

    let liquidity = u128::from_le_bytes(data[49..65].try_into().ok()?);
    let sqrt_price = u128::from_le_bytes(data[65..81].try_into().ok()?);
    let tick = i32::from_le_bytes(data[81..85].try_into().ok()?);
    let mint_a = Pubkey::try_from(&data[101..133]).ok()?;
    let vault_a = Pubkey::try_from(&data[133..165]).ok()?;
    let mint_b = Pubkey::try_from(&data[181..213]).ok()?;
    let vault_b = Pubkey::try_from(&data[213..245]).ok()?;

    let pool = PoolState {
        address: *pool_address,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: mint_a,
        token_b_mint: mint_b,
        token_a_reserve: 0,
        token_b_reserve: 0,
        fee_bps: DexType::OrcaWhirlpool.base_fee_bps(),
        current_tick: Some(tick),
        sqrt_price_x64: Some(sqrt_price),
        liquidity: Some(liquidity),
        last_slot: 0,
    };

    Some((pool, vault_a, vault_b))
}

/// Parse a Meteora DLMM LbPair account.
/// Returns (PoolState, reserve_x_pubkey, reserve_y_pubkey) or None if data is invalid.
///
/// Layout (8-byte Anchor discriminator, ~902-920 bytes depending on version):
///   offset 76: active_id (i32 LE, 4B)
///   offset 80: bin_step (u16 LE, 2B)
///   offset 88: token_x_mint (Pubkey, 32B)
///   offset 120: token_y_mint (Pubkey, 32B)
///   offset 152: reserve_x (Pubkey, 32B)
///   offset 184: reserve_y (Pubkey, 32B)
pub fn parse_meteora_dlmm_pool(
    pool_address: &Pubkey,
    data: &[u8],
) -> Option<(PoolState, Pubkey, Pubkey)> {
    if data.len() < 216 {
        return None;
    }

    let _active_id = i32::from_le_bytes(data[76..80].try_into().ok()?);
    let _bin_step = u16::from_le_bytes(data[80..82].try_into().ok()?);
    let mint_x = Pubkey::try_from(&data[88..120]).ok()?;
    let mint_y = Pubkey::try_from(&data[120..152]).ok()?;
    let reserve_x = Pubkey::try_from(&data[152..184]).ok()?;
    let reserve_y = Pubkey::try_from(&data[184..216]).ok()?;

    let pool = PoolState {
        address: *pool_address,
        dex_type: DexType::MeteoraDlmm,
        token_a_mint: mint_x,
        token_b_mint: mint_y,
        token_a_reserve: 0,
        token_b_reserve: 0,
        fee_bps: DexType::MeteoraDlmm.base_fee_bps(),
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 0,
    };

    Some((pool, reserve_x, reserve_y))
}
```

- [ ] **Step 4: Export the module**

Update `src/state/mod.rs`:

```rust
pub mod cache;
pub mod blockhash;
pub mod bootstrap;

pub use cache::StateCache;
pub use blockhash::BlockhashCache;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test unit bootstrap -- --nocapture`

Expected: All 5 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add src/state/bootstrap.rs src/state/mod.rs tests/unit/bootstrap.rs tests/unit/mod.rs
git commit -m "feat: DEX pool account parsers for Raydium, Orca, Meteora"
```

---

### Task 4: Pool bootstrapping — RPC fetch + StateCache population

**Files:**
- Modify: `src/state/bootstrap.rs`

- [ ] **Step 1: Add the RPC bootstrapping function**

Append to `src/state/bootstrap.rs`:

```rust
use std::str::FromStr;
use std::time::Duration;

/// Bootstrap pool state from on-chain data via RPC.
///
/// Calls `getProgramAccounts` for each DEX program, parses pool accounts,
/// and populates the StateCache with pool state and vault→pool index.
/// Then fetches vault balances via `getMultipleAccounts` in batches.
pub async fn bootstrap_pools(
    client: &reqwest::Client,
    rpc_url: &str,
    state_cache: &StateCache,
) -> anyhow::Result<BootstrapStats> {
    let mut stats = BootstrapStats::default();
    let mut all_vaults: Vec<(Pubkey, Pubkey, bool)> = Vec::new(); // (vault, pool, is_token_a)

    // Raydium AMM v4
    match fetch_and_parse_raydium(client, rpc_url, state_cache).await {
        Ok(vaults) => {
            stats.raydium_pools = vaults.len() / 2; // 2 vaults per pool
            all_vaults.extend(vaults);
        }
        Err(e) => {
            error!("Raydium bootstrap failed: {}", e);
            stats.errors += 1;
        }
    }

    // Orca Whirlpool
    match fetch_and_parse_orca(client, rpc_url, state_cache).await {
        Ok(vaults) => {
            stats.orca_pools = vaults.len() / 2;
            all_vaults.extend(vaults);
        }
        Err(e) => {
            error!("Orca bootstrap failed: {}", e);
            stats.errors += 1;
        }
    }

    // Meteora DLMM
    match fetch_and_parse_meteora(client, rpc_url, state_cache).await {
        Ok(vaults) => {
            stats.meteora_pools = vaults.len() / 2;
            all_vaults.extend(vaults);
        }
        Err(e) => {
            error!("Meteora bootstrap failed: {}", e);
            stats.errors += 1;
        }
    }

    // Fetch vault balances in batches
    let vault_pubkeys: Vec<Pubkey> = all_vaults.iter().map(|(v, _, _)| *v).collect();
    match fetch_vault_balances(client, rpc_url, &vault_pubkeys, state_cache).await {
        Ok(count) => {
            stats.vaults_fetched = count;
        }
        Err(e) => {
            error!("Vault balance fetch failed: {}", e);
            stats.errors += 1;
        }
    }

    stats.total_pools = stats.raydium_pools + stats.orca_pools + stats.meteora_pools;
    info!(
        "Bootstrap complete: {} pools ({} Raydium, {} Orca, {} Meteora), {} vaults, {} errors",
        stats.total_pools, stats.raydium_pools, stats.orca_pools, stats.meteora_pools,
        stats.vaults_fetched, stats.errors,
    );

    Ok(stats)
}

#[derive(Debug, Default)]
pub struct BootstrapStats {
    pub raydium_pools: usize,
    pub orca_pools: usize,
    pub meteora_pools: usize,
    pub total_pools: usize,
    pub vaults_fetched: usize,
    pub errors: usize,
}

// ── Per-DEX fetchers ──────────────────────────────────────────────

async fn fetch_and_parse_raydium(
    client: &reqwest::Client,
    rpc_url: &str,
    state_cache: &StateCache,
) -> anyhow::Result<Vec<(Pubkey, Pubkey, bool)>> {
    let program_id = crate::config::programs::raydium_amm();

    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getProgramAccounts",
        "params": [
            program_id.to_string(),
            {
                "encoding": "base64",
                "filters": [
                    { "dataSize": 752 },
                    { "memcmp": { "offset": 0, "bytes": "BgAAAAAAAAA=", "encoding": "base64" } }
                ]
            }
        ]
    });

    let accounts = rpc_get_program_accounts(client, rpc_url, &payload).await?;
    let mut vaults = Vec::new();

    for (pubkey, data) in &accounts {
        if let Some((pool, vault_a, vault_b)) = parse_raydium_amm_pool(pubkey, data) {
            state_cache.upsert(*pubkey, pool);
            state_cache.register_vault(vault_a, *pubkey, true);
            state_cache.register_vault(vault_b, *pubkey, false);
            vaults.push((vault_a, *pubkey, true));
            vaults.push((vault_b, *pubkey, false));
        }
    }

    info!("Raydium: parsed {} pools from {} accounts", vaults.len() / 2, accounts.len());
    Ok(vaults)
}

async fn fetch_and_parse_orca(
    client: &reqwest::Client,
    rpc_url: &str,
    state_cache: &StateCache,
) -> anyhow::Result<Vec<(Pubkey, Pubkey, bool)>> {
    let program_id = crate::config::programs::orca_whirlpool();

    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getProgramAccounts",
        "params": [
            program_id.to_string(),
            {
                "encoding": "base64",
                "filters": [
                    { "dataSize": 653 }
                ]
            }
        ]
    });

    let accounts = rpc_get_program_accounts(client, rpc_url, &payload).await?;
    let mut vaults = Vec::new();

    for (pubkey, data) in &accounts {
        if let Some((pool, vault_a, vault_b)) = parse_orca_whirlpool_pool(pubkey, data) {
            state_cache.upsert(*pubkey, pool);
            state_cache.register_vault(vault_a, *pubkey, true);
            state_cache.register_vault(vault_b, *pubkey, false);
            vaults.push((vault_a, *pubkey, true));
            vaults.push((vault_b, *pubkey, false));
        }
    }

    info!("Orca: parsed {} pools from {} accounts", vaults.len() / 2, accounts.len());
    Ok(vaults)
}

async fn fetch_and_parse_meteora(
    client: &reqwest::Client,
    rpc_url: &str,
    state_cache: &StateCache,
) -> anyhow::Result<Vec<(Pubkey, Pubkey, bool)>> {
    let program_id = crate::config::programs::meteora_dlmm();

    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getProgramAccounts",
        "params": [
            program_id.to_string(),
            {
                "encoding": "base64",
                "filters": []
            }
        ]
    });

    let accounts = rpc_get_program_accounts(client, rpc_url, &payload).await?;
    let mut vaults = Vec::new();

    for (pubkey, data) in &accounts {
        if let Some((pool, vault_a, vault_b)) = parse_meteora_dlmm_pool(pubkey, data) {
            state_cache.upsert(*pubkey, pool);
            state_cache.register_vault(vault_a, *pubkey, true);
            state_cache.register_vault(vault_b, *pubkey, false);
            vaults.push((vault_a, *pubkey, true));
            vaults.push((vault_b, *pubkey, false));
        }
    }

    info!("Meteora: parsed {} pools from {} accounts", vaults.len() / 2, accounts.len());
    Ok(vaults)
}

// ── RPC helpers ───────────────────────────────────────────────────

/// Call getProgramAccounts and return (pubkey, decoded_data) pairs.
async fn rpc_get_program_accounts(
    client: &reqwest::Client,
    rpc_url: &str,
    payload: &serde_json::Value,
) -> anyhow::Result<Vec<(Pubkey, Vec<u8>)>> {
    use base64::{engine::general_purpose, Engine as _};

    let resp = client
        .post(rpc_url)
        .json(payload)
        .timeout(Duration::from_secs(60))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let accounts = resp
        .get("result")
        .and_then(|r| r.as_array())
        .ok_or_else(|| anyhow::anyhow!("Invalid getProgramAccounts response"))?;

    let mut results = Vec::with_capacity(accounts.len());

    for account in accounts {
        let pubkey_str = account
            .get("pubkey")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing pubkey in account"))?;

        let pubkey = Pubkey::from_str(pubkey_str)?;

        let data_b64 = account
            .get("account")
            .and_then(|a| a.get("data"))
            .and_then(|d| d.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing account data"))?;

        let data = general_purpose::STANDARD.decode(data_b64)?;
        results.push((pubkey, data));
    }

    Ok(results)
}

/// Fetch SPL Token vault balances via `getMultipleAccounts` in batches of 100.
/// Updates vault balances in the StateCache.
async fn fetch_vault_balances(
    client: &reqwest::Client,
    rpc_url: &str,
    vault_pubkeys: &[Pubkey],
    state_cache: &StateCache,
) -> anyhow::Result<usize> {
    use base64::{engine::general_purpose, Engine as _};

    let mut total_updated = 0;

    for chunk in vault_pubkeys.chunks(100) {
        let keys: Vec<String> = chunk.iter().map(|p| p.to_string()).collect();

        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getMultipleAccounts",
            "params": [
                keys,
                { "encoding": "base64" }
            ]
        });

        let resp = client
            .post(rpc_url)
            .json(&payload)
            .timeout(Duration::from_secs(30))
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let slot = resp
            .get("result")
            .and_then(|r| r.get("context"))
            .and_then(|c| c.get("slot"))
            .and_then(|s| s.as_u64())
            .unwrap_or(0);

        let values = resp
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("Invalid getMultipleAccounts response"))?;

        for (i, value) in values.iter().enumerate() {
            if value.is_null() {
                continue; // Account doesn't exist
            }

            let data_b64 = value
                .get("data")
                .and_then(|d| d.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.as_str());

            if let Some(b64) = data_b64 {
                if let Ok(data) = general_purpose::STANDARD.decode(b64) {
                    // SPL Token account: balance at bytes 64..72 (u64 LE)
                    if data.len() >= 72 {
                        let balance = u64::from_le_bytes(
                            data[64..72].try_into().unwrap_or_default()
                        );
                        state_cache.update_vault_balance(&chunk[i], balance, slot);
                        total_updated += 1;
                    }
                }
            }
        }

        debug!("Fetched vault balances: batch of {}, {} updated", chunk.len(), total_updated);
    }

    Ok(total_updated)
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check`

Expected: Compiles cleanly.

- [ ] **Step 3: Commit**

```bash
git add src/state/bootstrap.rs
git commit -m "feat: pool bootstrapping via getProgramAccounts + vault balance fetch"
```

---

### Task 5: main.rs — call bootstrap at startup

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add bootstrap call after state cache creation**

In `src/main.rs`, after the Sanctum bootstrap block and before the shutdown signal, add:

```rust
    // Bootstrap DEX pool state from on-chain data
    info!("Bootstrapping pool state from RPC...");
    match state::bootstrap::bootstrap_pools(&http_client, &config.rpc_url, &state_cache).await {
        Ok(stats) => {
            info!(
                "Pool bootstrap complete: {} pools, {} vaults",
                stats.total_pools, stats.vaults_fetched
            );
            if stats.total_pools == 0 {
                warn!("WARNING: Zero pools bootstrapped — Geyser events may all be dropped");
            }
        }
        Err(e) => {
            error!("Pool bootstrap failed: {}", e);
            warn!("Continuing without bootstrap — Geyser will have a cold start");
        }
    }
```

Note: The `http_client` was created in Task 2 (blockhash integration). Move its creation to before both the blockhash fetch and the bootstrap call:

Move this block to right after `let state_cache = ...` and before the Sanctum bootstrap:

```rust
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .pool_max_idle_per_host(4)
        .build()?;
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check`

Expected: Compiles cleanly.

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat: call pool bootstrap at startup before Geyser stream"
```

---

### Task 6: Geyser stream reconnect with exponential backoff

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Replace the Geyser stream spawn with a reconnect loop**

In `src/main.rs`, replace the current Task 1 Geyser streaming block:

```rust
    // Task 1: Geyser streaming (async, I/O bound)
    let stream_handle = {
        let shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            if let Err(e) = geyser_stream.start(change_tx, shutdown_rx).await {
                error!("Geyser stream error: {}", e);
            }
        })
    };
```

With:

```rust
    // Task 1: Geyser streaming with reconnect (async, I/O bound)
    let stream_handle = {
        let shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            let mut backoff = std::time::Duration::from_secs(1);
            const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);

            loop {
                match geyser_stream.start(change_tx.clone(), shutdown_rx.clone()).await {
                    Ok(()) => {
                        info!("Geyser stream ended cleanly");
                    }
                    Err(e) => {
                        error!("Geyser stream error: {}", e);
                    }
                }

                // Check if shutdown was requested
                if *shutdown_rx.borrow() {
                    info!("Geyser: shutdown requested, not reconnecting");
                    break;
                }

                warn!("Geyser disconnected, reconnecting in {:?}...", backoff);
                tokio::time::sleep(backoff).await;

                // Check shutdown again after sleep
                if *shutdown_rx.borrow() {
                    break;
                }

                info!("Geyser: attempting reconnect (backoff {:?})...", backoff);
                backoff = std::cmp::min(backoff * 2, MAX_BACKOFF);
            }
        })
    };
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check`

Expected: Compiles cleanly.

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat: Geyser stream reconnect with exponential backoff"
```

---

### Task 7: Final integration — verify compilation, all tests, clippy

**Files:**
- None (verification only)

- [ ] **Step 1: Run cargo check**

Run: `cargo check`

Expected: Compiles cleanly.

- [ ] **Step 2: Run all unit tests**

Run: `cargo test --test unit -- --nocapture`

Expected: All tests PASS (previous 15 + 4 new blockhash + 5 new bootstrap = 24 total).

- [ ] **Step 3: Run all e2e tests**

Run: `cargo test --features e2e --test e2e -- --nocapture`

Expected: All 4 tests PASS.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -- -D warnings 2>&1 | grep "^error" || echo "No clippy errors"`

Fix any new warnings introduced by our changes. Pre-existing warnings in unchanged files can be ignored.

- [ ] **Step 5: Commit any fixes**

```bash
git add -A
git commit -m "chore: clippy fixes for Phase 1 completion"
```

(Only if Step 4 required changes.)

- [ ] **Step 6: Push**

```bash
git push origin main
```
