# DEX Module Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split 3 monolith DEX files into per-DEX modules following the relays/ pattern

**Architecture:** Extract per-DEX functions from bundle.rs (IX builders), stream.rs (parsers), and pool.rs (quoting math) into dedicated sub-modules under executor/swaps/, mempool/parsers/, and router/dex/. Each module has a mod.rs with shared helpers and a dispatcher. Pure move refactor — zero behavior change.

**Tech Stack:** Rust, no new dependencies

**Spec:** `docs/superpowers/specs/2026-04-16-dex-module-refactor-design.md`

---

### Task 1: Create `executor/swaps/` Module — Scaffold + First DEX

**Files:**
- Create: `src/executor/swaps/mod.rs`
- Create: `src/executor/swaps/raydium_amm.rs`
- Modify: `src/executor/mod.rs`
- Modify: `src/executor/bundle.rs`

This task sets up the module pattern and moves the first IX builder. Remaining DEXes follow the same pattern in Task 2.

- [ ] **Step 1: Create `src/executor/swaps/mod.rs`**

```rust
//! Per-DEX swap instruction builders.
//!
//! Each file builds the swap instruction for one DEX type.
//! The build_swap_ix() dispatcher routes by DexType.

pub mod raydium_amm;

// Re-export all builders for use in bundle.rs
pub use raydium_amm::build_raydium_amm_swap_ix;
```

- [ ] **Step 2: Create `src/executor/swaps/raydium_amm.rs`**

Move `build_raydium_amm_swap_ix()` (bundle.rs lines 516-565) into this file. Copy the function exactly as-is, adding the necessary `use` imports at the top:

```rust
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};
use crate::addresses;
use crate::router::pool::PoolState;
```

Then paste the full `build_raydium_amm_swap_ix` function body unchanged.

- [ ] **Step 3: Update `src/executor/mod.rs`**

Add the new module:

```rust
pub mod bundle;
pub mod confirmation;
pub mod relays;
pub mod relay_dispatcher;
pub mod swaps;
```

- [ ] **Step 4: Update `bundle.rs` — replace inline function with re-export**

Remove the `build_raydium_amm_swap_ix` function body from bundle.rs (lines 516-565). Replace the existing call site with `use crate::executor::swaps::build_raydium_amm_swap_ix;` at the top of bundle.rs. The match arm in `build_swap_instruction_with_min_out` already calls `build_raydium_amm_swap_ix(...)` — it just needs the import to resolve.

- [ ] **Step 5: Verify**

Run: `cargo test`
Expected: 242 passed, 0 failed

Run: `cargo clippy`
Expected: 0 errors

- [ ] **Step 6: Commit**

```bash
git add src/executor/swaps/ src/executor/mod.rs src/executor/bundle.rs
git commit -m "refactor: extract raydium_amm swap IX builder to executor/swaps/"
```

---

### Task 2: Move All Remaining Swap IX Builders to `executor/swaps/`

**Files:**
- Create: `src/executor/swaps/raydium_cp.rs`
- Create: `src/executor/swaps/raydium_clmm.rs`
- Create: `src/executor/swaps/orca.rs`
- Create: `src/executor/swaps/meteora_dlmm.rs`
- Create: `src/executor/swaps/meteora_damm_v2.rs`
- Create: `src/executor/swaps/sanctum.rs`
- Create: `src/executor/swaps/phoenix.rs`
- Create: `src/executor/swaps/manifest.rs`
- Create: `src/executor/swaps/pumpswap.rs`
- Modify: `src/executor/swaps/mod.rs`
- Modify: `src/executor/bundle.rs`

Repeat the Task 1 pattern for each remaining DEX. For each file:

1. Create the file with the necessary `use` imports (`solana_sdk`, `crate::addresses`, `crate::router::pool::PoolState`)
2. Move the function body from bundle.rs unchanged
3. Add `pub mod <name>;` and `pub use <name>::<fn>;` to swaps/mod.rs
4. Remove the function from bundle.rs

**Function → file mapping (bundle.rs line numbers):**

| Function | Lines | Target file | Extra deps |
|----------|-------|-------------|------------|
| `build_raydium_cp_swap_ix` | 621-678 | `raydium_cp.rs` | — |
| `build_raydium_clmm_swap_ix` | 843-945 | `raydium_clmm.rs` | `floor_div` helper (move to mod.rs) |
| `build_orca_whirlpool_swap_ix` | 752-841 | `orca.rs` | `floor_div` helper |
| `build_damm_v2_swap_ix` | 680-735 | `meteora_damm_v2.rs` | — |
| `build_meteora_dlmm_swap_ix` | 947-1065 | `meteora_dlmm.rs` | — |
| `build_sanctum_swap_ix` | 567-619 | `sanctum.rs` | Also move `sanctum_swap_accounts_v2` (lines 1288-end) |
| `build_phoenix_swap_ix` | 1067-1123 | `phoenix.rs` | — |
| `build_manifest_swap_ix` | 1125-1172 | `manifest.rs` | — |
| `build_pumpswap_swap_ix` | 1174-1287 | `pumpswap.rs` | — |

**Shared helper `floor_div`** (bundle.rs line 737): move to `swaps/mod.rs` and make `pub(crate)`. Used by `orca.rs` and `raydium_clmm.rs` for tick array index calculation.

- [ ] **Step 1: Move `floor_div` to `swaps/mod.rs`**

Add to mod.rs:
```rust
/// Floor division for tick array index calculation.
/// Used by Orca and Raydium CLMM IX builders.
pub(crate) fn floor_div(dividend: i32, divisor: i32) -> i32 {
    // exact same body from bundle.rs line 737
}
```

- [ ] **Step 2: Create all 9 per-DEX files**

Each file follows this template (example for `raydium_cp.rs`):
```rust
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};
use crate::addresses;
use crate::router::pool::PoolState;

/// Build a Raydium CP-Swap instruction with the full 13-account layout.
pub fn build_raydium_cp_swap_ix(
    // exact same signature and body from bundle.rs
) -> Option<Instruction> {
    // exact same body
}
```

For files needing `floor_div` (orca.rs, raydium_clmm.rs), import via:
```rust
use super::floor_div;
```

For `sanctum.rs`, also move the private helper `sanctum_swap_accounts_v2`.

- [ ] **Step 3: Update `swaps/mod.rs` with all modules and re-exports**

```rust
pub mod raydium_amm;
pub mod raydium_cp;
pub mod raydium_clmm;
pub mod orca;
pub mod meteora_dlmm;
pub mod meteora_damm_v2;
pub mod sanctum;
pub mod phoenix;
pub mod manifest;
pub mod pumpswap;

pub use raydium_amm::build_raydium_amm_swap_ix;
pub use raydium_cp::build_raydium_cp_swap_ix;
pub use raydium_clmm::build_raydium_clmm_swap_ix;
pub use orca::build_orca_whirlpool_swap_ix;
pub use meteora_dlmm::build_meteora_dlmm_swap_ix;
pub use meteora_damm_v2::build_damm_v2_swap_ix;
pub use sanctum::build_sanctum_swap_ix;
pub use phoenix::build_phoenix_swap_ix;
pub use manifest::build_manifest_swap_ix;
pub use pumpswap::build_pumpswap_swap_ix;

pub(crate) fn floor_div(dividend: i32, divisor: i32) -> i32 {
    // body
}
```

- [ ] **Step 4: Update bundle.rs imports**

Replace all removed function bodies with imports from swaps:
```rust
use crate::executor::swaps::{
    build_raydium_amm_swap_ix, build_raydium_cp_swap_ix, build_raydium_clmm_swap_ix,
    build_orca_whirlpool_swap_ix, build_meteora_dlmm_swap_ix, build_damm_v2_swap_ix,
    build_sanctum_swap_ix, build_phoenix_swap_ix, build_manifest_swap_ix,
    build_pumpswap_swap_ix,
};
```

Remove `floor_div` and `sanctum_swap_accounts_v2` from bundle.rs. The `derive_ata` / `derive_ata_with_program` / `estimate_unique_accounts` functions STAY in bundle.rs.

- [ ] **Step 5: Verify**

Run: `cargo test`
Expected: 242 passed, 0 failed

Run: `cargo clippy`
Expected: 0 errors

- [ ] **Step 6: Commit**

```bash
git add src/executor/swaps/ src/executor/bundle.rs
git commit -m "refactor: extract all swap IX builders to executor/swaps/"
```

---

### Task 3: Create `mempool/parsers/` Module — Scaffold + Shared Helpers

**Files:**
- Create: `src/mempool/parsers/mod.rs`
- Modify: `src/mempool/mod.rs`

- [ ] **Step 1: Create `src/mempool/parsers/mod.rs`**

Move shared helpers from stream.rs and set up the module structure:

```rust
//! Per-DEX Geyser pool state parsers.
//!
//! Each file parses one DEX's pool account data into PoolState.
//! Shared helpers (reserve approximation, floor_div) live here.

pub mod orca;
pub mod raydium_amm;
pub mod raydium_cp;
pub mod raydium_clmm;
pub mod meteora_dlmm;
pub mod meteora_damm_v2;
pub mod phoenix;
pub mod manifest;
pub mod pumpswap;

pub use orca::parse_orca_whirlpool;
pub use raydium_amm::parse_raydium_amm_v4;
pub use raydium_cp::parse_raydium_cp;
pub use raydium_clmm::parse_raydium_clmm;
pub use meteora_dlmm::parse_meteora_dlmm;
pub use meteora_damm_v2::parse_meteora_damm_v2;
pub use phoenix::parse_phoenix_market;
pub use manifest::parse_manifest_market;
pub use pumpswap::parse_pumpswap;

// Also re-export the orderbook dispatcher used by stream.rs
pub use phoenix::try_parse_orderbook;

/// Approximate token reserves from CLMM sqrt_price and liquidity.
/// Used by Orca and Raydium CLMM parsers to derive reserve estimates.
/// (Exact copy from stream.rs line 970)
pub fn approx_reserves_from_sqrt_price(sqrt_price_x64: u128, liquidity: u128) -> (u64, u64) {
    // exact same body from stream.rs
}

/// Floor division helper for tick array indexing.
pub fn floor_div(dividend: i32, divisor: i32) -> i32 {
    if (dividend ^ divisor) < 0 && dividend % divisor != 0 {
        dividend / divisor - 1
    } else {
        dividend / divisor
    }
}
```

- [ ] **Step 2: Update `src/mempool/mod.rs`**

```rust
pub mod stream;
pub mod parsers;

pub use stream::{GeyserStream, PoolStateChange};
```

- [ ] **Step 3: Verify (will have compile errors — modules declared but files don't exist yet)**

This is expected — continue to Task 4.

---

### Task 4: Move All Parsers to `mempool/parsers/`

**Files:**
- Create: `src/mempool/parsers/orca.rs`
- Create: `src/mempool/parsers/raydium_amm.rs`
- Create: `src/mempool/parsers/raydium_cp.rs`
- Create: `src/mempool/parsers/raydium_clmm.rs`
- Create: `src/mempool/parsers/meteora_dlmm.rs`
- Create: `src/mempool/parsers/meteora_damm_v2.rs`
- Create: `src/mempool/parsers/phoenix.rs`
- Create: `src/mempool/parsers/manifest.rs`
- Create: `src/mempool/parsers/pumpswap.rs`
- Modify: `src/mempool/stream.rs`

**Function → file mapping (stream.rs line numbers):**

| Function | Lines | Target file | Shared deps |
|----------|-------|-------------|-------------|
| `parse_orca_whirlpool` | 1005-1064 | `orca.rs` | `approx_reserves_from_sqrt_price` from mod.rs |
| `parse_raydium_clmm` | 1066-1123 | `raydium_clmm.rs` | `approx_reserves_from_sqrt_price` from mod.rs |
| `parse_raydium_amm_v4` | 1254-1330 | `raydium_amm.rs` | — |
| `parse_raydium_cp` | 1332-1378 | `raydium_cp.rs` | — |
| `parse_meteora_dlmm` | 1125-1190 | `meteora_dlmm.rs` | — |
| `parse_meteora_damm_v2` | 1192-1252 | `meteora_damm_v2.rs` | — |
| `parse_phoenix_market` | 1478-1597 | `phoenix.rs` | Also move `try_parse_orderbook` (line 1380) and `phoenix_tree_best` (line 1411) |
| `parse_manifest_market` | 1599-1660 | `manifest.rs` | — |
| `parse_pumpswap` | 1661-end | `pumpswap.rs` | — |

- [ ] **Step 1: Create all 9 per-DEX parser files**

Each file follows this template (example for `orca.rs`):
```rust
use solana_sdk::pubkey::Pubkey;
use crate::addresses;
use crate::router::pool::{DexType, PoolExtra, PoolState};

/// Parse an Orca Whirlpool account (653 bytes, Anchor discriminator).
pub fn parse_orca_whirlpool(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<PoolState> {
    // exact same body from stream.rs
}
```

For files needing shared helpers:
```rust
use super::approx_reserves_from_sqrt_price; // orca.rs, raydium_clmm.rs
```

For `phoenix.rs`, also move `try_parse_orderbook` and `phoenix_tree_best`. Make `try_parse_orderbook` public so stream.rs can call it for variable-size account routing.

- [ ] **Step 2: Update stream.rs — replace inline functions with imports**

Add at top of stream.rs:
```rust
use crate::mempool::parsers;
```

Replace all direct calls like `parse_orca_whirlpool(...)` with `parsers::parse_orca_whirlpool(...)`, or add `use crate::mempool::parsers::*;` and keep calls unchanged.

Remove all moved function bodies and the `approx_reserves_from_sqrt_price` helper from stream.rs.

- [ ] **Step 3: Verify**

Run: `cargo test`
Expected: 242 passed, 0 failed

Run: `cargo clippy`
Expected: 0 errors

- [ ] **Step 4: Commit**

```bash
git add src/mempool/parsers/ src/mempool/mod.rs src/mempool/stream.rs
git commit -m "refactor: extract all Geyser parsers to mempool/parsers/"
```

---

### Task 5: Create `router/dex/` Module — Shared Math + CPMM

**Files:**
- Create: `src/router/dex/mod.rs`
- Create: `src/router/dex/cpmm.rs`
- Modify: `src/router/mod.rs`
- Modify: `src/router/pool.rs`

This is the most delicate extraction — quoting functions are methods on `PoolState`. Strategy: extract the math into free functions that take the necessary fields as parameters, and have the `PoolState` methods delegate.

- [ ] **Step 1: Create `src/router/dex/mod.rs`**

```rust
//! Per-DEX quoting math.
//!
//! Each file implements output amount calculation for one DEX type.
//! Shared math (ceil_div, tick_to_sqrt_price, compute_swap_step) lives here.

pub mod cpmm;
pub mod clmm_orca;
pub mod clmm_raydium;
pub mod dlmm;
pub mod damm_v2;
pub mod sanctum;
pub mod phoenix;
pub mod manifest;

/// Ceiling division for u128. Used by CLMM and DLMM quoting.
pub fn ceil_div(a: u128, b: u128) -> u128 {
    // exact same body from pool.rs line 766
}

/// Convert a tick index to sqrt_price in Q64.64 format.
/// Used by Orca and Raydium CLMM multi-tick crossing.
/// (exact copy from pool.rs line 867)
pub fn tick_index_to_sqrt_price_x64(tick: i32) -> Option<u128> {
    // exact same body
}

/// Result of a single CLMM swap step within one tick range.
pub struct SwapStepResult {
    pub sqrt_price_next: u128,
    pub amount_in: u128,
    pub amount_out: u128,
    pub fee_amount: u128,
}

/// Compute one swap step within a tick range.
/// Used by both Orca and Raydium CLMM multi-tick quoting.
/// (exact copy from pool.rs line 791)
pub fn compute_swap_step(
    sqrt_price_current: u128,
    sqrt_price_target: u128,
    liquidity: u128,
    amount_remaining: u128,
    fee_rate: u64,
) -> Option<SwapStepResult> {
    // exact same body
}
```

- [ ] **Step 2: Create `src/router/dex/cpmm.rs`**

```rust
//! Constant-product AMM quoting.
//! Used by: RaydiumAmm, RaydiumCp, PumpSwap, DammV2 (flat pools).

use crate::router::pool::PoolState;

/// Compute output for a constant-product swap.
/// Extracted from PoolState::get_cpmm_output().
pub fn quote(pool: &PoolState, input_amount: u64, a_to_b: bool) -> Option<u64> {
    // exact same body as get_cpmm_output (pool.rs lines 309-338)
    // but takes pool as parameter instead of &self
}
```

- [ ] **Step 3: Update `src/router/mod.rs`**

```rust
pub mod pool;
pub mod calculator;
pub mod simulator;
pub mod dex;
```

- [ ] **Step 4: Update `pool.rs` — delegate `get_cpmm_output` to dex::cpmm**

Replace the `get_cpmm_output` method body with:
```rust
fn get_cpmm_output(&self, input_amount: u64, a_to_b: bool) -> Option<u64> {
    crate::router::dex::cpmm::quote(self, input_amount, a_to_b)
}
```

- [ ] **Step 5: Verify**

Run: `cargo test`
Expected: 242 passed

- [ ] **Step 6: Commit**

```bash
git add src/router/dex/ src/router/mod.rs src/router/pool.rs
git commit -m "refactor: extract CPMM quoting + shared CLMM math to router/dex/"
```

---

### Task 6: Move Remaining Quoting Math to `router/dex/`

**Files:**
- Create: `src/router/dex/clmm_orca.rs`
- Create: `src/router/dex/clmm_raydium.rs`
- Create: `src/router/dex/dlmm.rs`
- Create: `src/router/dex/damm_v2.rs`
- Create: `src/router/dex/sanctum.rs`
- Create: `src/router/dex/phoenix.rs`
- Create: `src/router/dex/manifest.rs`
- Modify: `src/router/pool.rs`

**Method → file mapping (pool.rs line numbers):**

| Method | Lines | Target file | Shared deps |
|--------|-------|-------------|-------------|
| `get_clmm_output` (single tick) | 178-250 | Used as fallback by both `clmm_orca.rs` and `clmm_raydium.rs`. Move to `dex/mod.rs` as `clmm_single_tick_output()` |
| `get_clmm_multi_tick_output` | 397-533 | Split: Orca-specific logic to `clmm_orca.rs`, Raydium-specific to `clmm_raydium.rs`. They share `compute_swap_step` and `tick_index_to_sqrt_price_x64` from mod.rs |
| `get_orderbook_output` | 263-306 | Split: Phoenix to `phoenix.rs`, Manifest to `manifest.rs`. Both use the same formula with different price scaling (D18 for Manifest) |
| `get_dlmm_bin_output` | 535-764 | `dlmm.rs` — uses `ceil_div` from mod.rs |

- [ ] **Step 1: Move `get_clmm_output` (single-tick) to `dex/mod.rs`**

Add as a public free function:
```rust
/// Single-tick CLMM output calculation. Fallback when tick arrays aren't loaded.
pub fn clmm_single_tick_output(
    fee_bps: u16,
    sqrt_price_x64: u128,
    liquidity: u128,
    input_amount: u64,
    a_to_b: bool,
) -> Option<u64> {
    // exact body from pool.rs get_clmm_output, but parameterized
}
```

- [ ] **Step 2: Create `clmm_orca.rs` and `clmm_raydium.rs`**

Each gets the relevant portion of `get_clmm_multi_tick_output`. The two CLMM variants differ in:
- Tick spacing (Orca uses 88 default, Raydium uses pool-specific)
- Tick array layout offsets

```rust
// clmm_orca.rs
use crate::router::pool::{PoolState, ClmmTickArray};
use super::{compute_swap_step, tick_index_to_sqrt_price_x64, ceil_div, clmm_single_tick_output};

pub fn quote(
    pool: &PoolState,
    input_amount: u64,
    a_to_b: bool,
    tick_arrays: Option<&[ClmmTickArray]>,
) -> Option<u64> {
    // Try multi-tick first, fall back to single-tick
    if let Some(ticks) = tick_arrays {
        if let (Some(sqrt_price), Some(liquidity)) = (pool.sqrt_price_x64, pool.liquidity) {
            if !ticks.is_empty() && sqrt_price > 0 && liquidity > 0 {
                return multi_tick_output(pool, input_amount, a_to_b, sqrt_price, liquidity, ticks)
                    .or_else(|| clmm_single_tick_output(pool.fee_bps, sqrt_price, liquidity, input_amount, a_to_b));
            }
        }
    }
    // Single-tick fallback
    let (sqrt_price, liquidity) = (pool.sqrt_price_x64?, pool.liquidity?);
    if sqrt_price > 0 && liquidity > 0 {
        clmm_single_tick_output(pool.fee_bps, sqrt_price, liquidity, input_amount, a_to_b)
    } else {
        None
    }
}

fn multi_tick_output(/* same body as Orca portion of get_clmm_multi_tick_output */) -> Option<u64> {
    // ...
}
```

Same pattern for `clmm_raydium.rs`.

- [ ] **Step 3: Create `dlmm.rs`**

```rust
use crate::router::pool::{PoolState, DlmmBinArray};
use super::ceil_div;

pub fn quote(
    pool: &PoolState,
    input_amount: u64,
    a_to_b: bool,
    bin_arrays: Option<&[DlmmBinArray]>,
) -> Option<u64> {
    let active_id = pool.current_tick?;
    let bins = bin_arrays?;
    if bins.is_empty() { return None; }
    bin_by_bin_output(pool, input_amount, a_to_b, active_id, bins)
}

fn bin_by_bin_output(/* exact body from get_dlmm_bin_output */) -> Option<u64> {
    // ...
}
```

- [ ] **Step 4: Create `phoenix.rs` and `manifest.rs`**

Extract `get_orderbook_output` logic, parameterized per DEX:

```rust
// phoenix.rs
use crate::router::pool::PoolState;

pub fn quote(pool: &PoolState, input_amount: u64, a_to_b: bool) -> Option<u64> {
    // Orderbook quoting with raw price (not D18)
}

// manifest.rs
pub fn quote(pool: &PoolState, input_amount: u64, a_to_b: bool) -> Option<u64> {
    // Orderbook quoting with D18 price scaling
}
```

- [ ] **Step 5: Create `damm_v2.rs` and `sanctum.rs`**

DAMM v2 uses CPMM for flat pools but may have concentrated pools. Sanctum uses rate-based math. Both are straightforward extractions:

```rust
// damm_v2.rs — delegates to cpmm::quote for now
use crate::router::pool::PoolState;

pub fn quote(pool: &PoolState, input_amount: u64, a_to_b: bool) -> Option<u64> {
    super::cpmm::quote(pool, input_amount, a_to_b)
}
```

- [ ] **Step 6: Update `pool.rs` — rewrite `get_output_amount_with_cache` as dispatcher**

```rust
pub fn get_output_amount_with_cache(
    &self,
    input_amount: u64,
    a_to_b: bool,
    bin_arrays: Option<&[DlmmBinArray]>,
    tick_arrays: Option<&[ClmmTickArray]>,
) -> Option<u64> {
    if input_amount == 0 { return Some(0); }

    use crate::router::dex;
    match self.dex_type {
        DexType::MeteoraDlmm => dex::dlmm::quote(self, input_amount, a_to_b, bin_arrays)
            .or_else(|| dex::cpmm::quote(self, input_amount, a_to_b)),
        DexType::OrcaWhirlpool => dex::clmm_orca::quote(self, input_amount, a_to_b, tick_arrays)
            .or_else(|| dex::cpmm::quote(self, input_amount, a_to_b)),
        DexType::RaydiumClmm => dex::clmm_raydium::quote(self, input_amount, a_to_b, tick_arrays)
            .or_else(|| dex::cpmm::quote(self, input_amount, a_to_b)),
        DexType::Phoenix => dex::phoenix::quote(self, input_amount, a_to_b),
        DexType::Manifest => dex::manifest::quote(self, input_amount, a_to_b),
        DexType::MeteoraDammV2 => dex::damm_v2::quote(self, input_amount, a_to_b),
        DexType::SanctumInfinity => dex::sanctum::quote(self, input_amount, a_to_b),
        _ => dex::cpmm::quote(self, input_amount, a_to_b),
    }
}
```

Also simplify `get_output_amount()` to dispatch through `get_output_amount_with_cache(input, a_to_b, None, None)`.

Remove the old method bodies: `get_cpmm_output`, `get_clmm_output`, `get_orderbook_output`, `get_clmm_multi_tick_output`, `get_dlmm_bin_output`, `ceil_div`, `compute_swap_step`, `tick_index_to_sqrt_price_x64`.

- [ ] **Step 7: Verify**

Run: `cargo test`
Expected: 242 passed, 0 failed

Run: `cargo clippy`
Expected: 0 errors

- [ ] **Step 8: Commit**

```bash
git add src/router/dex/ src/router/pool.rs src/router/mod.rs
git commit -m "refactor: extract all quoting math to router/dex/"
```

---

### Task 7: Final Verification + Cleanup

**Files:**
- Modify: `CLAUDE.md` (update module map)

- [ ] **Step 1: Run full test suite**

```bash
cargo test
```
Expected: 242 passed, 0 failed

- [ ] **Step 2: Run clippy**

```bash
cargo clippy -- -D warnings
```
Expected: 0 warnings

- [ ] **Step 3: Build release**

```bash
cargo build --release
```
Expected: compiles cleanly

- [ ] **Step 4: Run engine for 2 minutes**

```bash
timeout 120 cargo run --release --bin solana-mev-bot
```
Expected: identical behavior — opportunities detected, bundles submitted, same log patterns.

- [ ] **Step 5: Update CLAUDE.md module map**

Update the module map section to reflect the new structure:

```
src/
├── executor/
│   ├── bundle.rs          # BundleBuilder, execute_arb_v2, ATA/wSOL logic
│   ├── swaps/             # Per-DEX swap IX builders
│   │   ├── mod.rs         # Dispatcher + shared helpers (floor_div)
│   │   ├── raydium_amm.rs
│   │   ├── raydium_cp.rs
│   │   ├── raydium_clmm.rs
│   │   ├── orca.rs
│   │   ├── meteora_dlmm.rs
│   │   ├── meteora_damm_v2.rs
│   │   ├── sanctum.rs
│   │   ├── phoenix.rs
│   │   ├── manifest.rs
│   │   └── pumpswap.rs
│   ├── relays/            # (unchanged)
│   └── ...
├── mempool/
│   ├── stream.rs          # GeyserStream, LaserStream subscription, lazy fetches
│   └── parsers/           # Per-DEX Geyser pool state parsers
│       ├── mod.rs         # Shared: approx_reserves, floor_div, dispatcher
│       ├── orca.rs
│       ├── raydium_amm.rs
│       ├── raydium_cp.rs
│       ├── raydium_clmm.rs
│       ├── meteora_dlmm.rs
│       ├── meteora_damm_v2.rs
│       ├── phoenix.rs
│       ├── manifest.rs
│       └── pumpswap.rs
├── router/
│   ├── pool.rs            # PoolState, DexType, ArbRoute (types only)
│   ├── dex/               # Per-DEX quoting math
│   │   ├── mod.rs         # Shared: ceil_div, tick_to_sqrt, compute_swap_step, cpmm
│   │   ├── cpmm.rs
│   │   ├── clmm_orca.rs
│   │   ├── clmm_raydium.rs
│   │   ├── dlmm.rs
│   │   ├── damm_v2.rs
│   │   ├── sanctum.rs
│   │   ├── phoenix.rs
│   │   └── manifest.rs
│   ├── calculator.rs
│   └── simulator.rs
└── ...
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: complete DEX module split — update docs"
```
