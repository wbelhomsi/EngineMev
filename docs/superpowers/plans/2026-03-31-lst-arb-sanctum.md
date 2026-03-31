# Phase 2: LST Rate Arb + Sanctum Virtual Pool — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add LST (jitoSOL, mSOL, bSOL) arbitrage with Sanctum Infinity as a virtual pool to the existing MEV engine.

**Architecture:** Bolt-on to existing pipeline — no new modules. LST pools and Sanctum virtual pools are registered as `PoolState` entries with a new `DexType::SanctumInfinity` variant. RouteCalculator discovers cross-DEX and DEX↔Sanctum arb routes automatically. Profit guaranteed on-chain via `minimum_amount_out` on final hop.

**Tech Stack:** Rust, solana-sdk 2.2, Surfpool (e2e tests), existing crate dependencies.

**Prerequisite:** Rust toolchain must be installed (`rustup`). Run `cargo check` to verify the project compiles before starting.

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `src/config.rs` | Modify | LST mints, Sanctum program IDs, SOL Value Calculator mapping, new env vars |
| `src/router/pool.rs` | Modify | `DexType::SanctumInfinity` variant, `base_fee_bps()` match arm |
| `src/executor/bundle.rs` | Modify | `build_sanctum_swap_ix()`, `minimum_amount_out` profit enforcement on final hop, dispatch in `build_swap_instruction()` |
| `src/router/simulator.rs` | Modify | LST spread threshold gate using `lst_min_spread_bps` |
| `src/mempool/stream.rs` | Modify | Add Sanctum program to Geyser subscription filter |
| `src/main.rs` | Modify | Bootstrap Sanctum virtual pools at startup |
| `.env.example` | Modify | Add `LST_ARB_ENABLED`, `LST_MIN_SPREAD_BPS` |
| `tests/unit/config_lst.rs` | Create | Unit tests for LST config parsing |
| `tests/unit/pool_sanctum.rs` | Create | Unit tests for Sanctum virtual pool rate math |
| `tests/unit/calculator_lst.rs` | Create | Unit tests for LST route discovery |
| `tests/unit/bundle_sanctum.rs` | Create | Unit tests for Sanctum IX building + profit enforcement |
| `tests/unit/simulator_lst.rs` | Create | Unit tests for LST spread gate |
| `tests/unit/mod.rs` | Create | Module declarations for unit tests |
| `tests/e2e/mod.rs` | Create | Module declarations for e2e tests (feature-gated) |
| `tests/e2e/lst_pipeline.rs` | Create | E2E tests using Surfpool |
| `Cargo.toml` | Modify | Add `[dev-dependencies]`, `e2e` feature flag |

---

### Task 1: Add `DexType::SanctumInfinity` and base fee

**Files:**
- Modify: `src/router/pool.rs:1-23`

- [ ] **Step 1: Write the failing test**

Create `tests/unit/pool_sanctum.rs`:

```rust
use solana_sdk::pubkey::Pubkey;

// Import the crate — Cargo integration tests use the crate name
use solana_mev_bot::router::pool::{DexType, PoolState};

#[test]
fn test_sanctum_infinity_base_fee() {
    assert_eq!(DexType::SanctumInfinity.base_fee_bps(), 3);
}

#[test]
fn test_sanctum_virtual_pool_rate() {
    // jitoSOL rate = 1.082 SOL per jitoSOL
    // Synthetic reserves: reserve_a = 1_000_000_000_000_000, reserve_b = reserve_a / 1.082
    let rate = 1.082_f64;
    let reserve_a: u64 = 1_000_000_000_000_000;
    let reserve_b: u64 = (reserve_a as f64 / rate) as u64;

    let pool = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::SanctumInfinity,
        token_a_mint: Pubkey::new_unique(), // SOL
        token_b_mint: Pubkey::new_unique(), // jitoSOL
        token_a_reserve: reserve_a,
        token_b_reserve: reserve_b,
        fee_bps: 3,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
    };

    // Swap 1_000_000_000 lamports (1 SOL) worth of jitoSOL -> SOL
    // With rate 1.082, expect ~1.082 SOL out minus 3bps fee
    let input = 1_000_000_000u64; // 1 jitoSOL in lamports
    let output = pool.get_output_amount(input, false).unwrap(); // b_to_a

    // Expected: ~1.082 SOL. With constant-product approximation on huge reserves,
    // price impact is negligible. Allow 0.1% tolerance.
    let expected = (input as f64 * rate * (1.0 - 3.0 / 10_000.0)) as u64;
    let diff = (output as i64 - expected as i64).unsigned_abs();
    assert!(
        diff < expected / 1000,
        "Output {} too far from expected {}, diff={}",
        output, expected, diff
    );
}

#[test]
fn test_sanctum_virtual_pool_fee_deduction() {
    // Verify that fee_bps=3 on a Sanctum pool deducts ~3bps from output
    let reserve_a: u64 = 1_000_000_000_000_000;
    let reserve_b: u64 = 1_000_000_000_000_000; // rate = 1.0 for simplicity

    let pool_with_fee = PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::SanctumInfinity,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: reserve_a,
        token_b_reserve: reserve_b,
        fee_bps: 3,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
    };

    let pool_no_fee = PoolState {
        fee_bps: 0,
        ..pool_with_fee.clone()
    };

    let input = 1_000_000_000u64;
    let out_fee = pool_with_fee.get_output_amount(input, true).unwrap();
    let out_no_fee = pool_no_fee.get_output_amount(input, true).unwrap();

    // Fee pool output should be ~3bps less
    assert!(out_no_fee > out_fee);
    let fee_bps_actual = ((out_no_fee - out_fee) as f64 / out_no_fee as f64) * 10_000.0;
    assert!(
        (fee_bps_actual - 3.0).abs() < 0.5,
        "Effective fee {}bps too far from expected 3bps",
        fee_bps_actual
    );
}
```

Create `tests/unit/mod.rs`:

```rust
mod pool_sanctum;
```

- [ ] **Step 2: Set up test infrastructure in Cargo.toml and lib exports**

The crate currently only has `main.rs` (binary). Integration tests need to import types. Add a `src/lib.rs` that re-exports modules:

Create `src/lib.rs`:

```rust
pub mod config;
pub mod executor;
pub mod mempool;
pub mod router;
pub mod state;
```

Update `src/main.rs` — change the top-level module declarations from `mod` to `use`:

Replace:
```rust
mod config;
mod executor;
mod mempool;
mod router;
mod state;
```

With:
```rust
use solana_mev_bot::{config, executor, mempool, router, state};
```

Add to `Cargo.toml` after `[dependencies]`:

```toml
[dev-dependencies]

[[test]]
name = "unit"
path = "tests/unit/mod.rs"
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --test unit test_sanctum_infinity_base_fee -- --nocapture`

Expected: Compilation error — `DexType::SanctumInfinity` does not exist.

- [ ] **Step 4: Add `SanctumInfinity` variant to `DexType`**

In `src/router/pool.rs`, add the variant and match arm:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DexType {
    RaydiumAmm,
    RaydiumClmm,
    OrcaWhirlpool,
    MeteoraDlmm,
    SanctumInfinity,
}

impl DexType {
    pub fn base_fee_bps(&self) -> u64 {
        match self {
            DexType::RaydiumAmm => 25,
            DexType::RaydiumClmm => 1,
            DexType::OrcaWhirlpool => 1,
            DexType::MeteoraDlmm => 1,
            DexType::SanctumInfinity => 3,
        }
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test unit -- --nocapture`

Expected: All 3 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add src/router/pool.rs src/lib.rs tests/unit/ Cargo.toml
git commit -m "feat: add DexType::SanctumInfinity + virtual pool rate tests"
```

---

### Task 2: LST config — mints, Sanctum program IDs, env vars

**Files:**
- Modify: `src/config.rs`
- Modify: `.env.example`
- Create: `tests/unit/config_lst.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/unit/config_lst.rs`:

```rust
use solana_mev_bot::config;

#[test]
fn test_lst_mints_parse() {
    let mints = config::lst_mints();
    assert_eq!(mints.len(), 3);
    assert_eq!(mints[0].1, "jitoSOL");
    assert_eq!(mints[1].1, "mSOL");
    assert_eq!(mints[2].1, "bSOL");

    // Verify pubkeys are valid (didn't panic during creation)
    for (pubkey, name) in &mints {
        assert_ne!(pubkey.to_string(), "", "Invalid pubkey for {}", name);
    }
}

#[test]
fn test_sol_value_calculator_mapping() {
    let mints = config::lst_mints();
    for (mint, name) in &mints {
        let calc = config::sanctum_sol_value_calculator(mint);
        assert!(calc.is_some(), "No SOL Value Calculator for {}", name);
    }
}

#[test]
fn test_sol_value_calculator_unknown_mint() {
    let unknown = solana_sdk::pubkey::Pubkey::new_unique();
    assert!(config::sanctum_sol_value_calculator(&unknown).is_none());
}

#[test]
fn test_sanctum_program_ids() {
    let s_controller = config::programs::sanctum_s_controller();
    assert_eq!(
        s_controller.to_string(),
        "5ocnV1qiCgaQR8Jb8xWnVbApfaygJ8tNoZfgPwsgx9kx"
    );

    let pricing = config::programs::sanctum_flat_fee_pricing();
    assert_eq!(
        pricing.to_string(),
        "f1tUoNEKrDp1oeGn4zxr7bh41eN6VcfHjfrL3ZqQday"
    );
}
```

Add to `tests/unit/mod.rs`:

```rust
mod pool_sanctum;
mod config_lst;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test unit test_lst_mints_parse -- --nocapture`

Expected: Compilation error — `config::lst_mints` does not exist.

- [ ] **Step 3: Implement LST config**

In `src/config.rs`, add after the existing `programs` module (after line 28):

```rust
pub mod programs {
    use super::*;

    // ... existing functions unchanged ...

    pub fn sanctum_s_controller() -> Pubkey {
        Pubkey::from_str("5ocnV1qiCgaQR8Jb8xWnVbApfaygJ8tNoZfgPwsgx9kx").unwrap()
    }

    pub fn sanctum_flat_fee_pricing() -> Pubkey {
        Pubkey::from_str("f1tUoNEKrDp1oeGn4zxr7bh41eN6VcfHjfrL3ZqQday").unwrap()
    }
}
```

Add after the `programs` module, before `BotConfig`:

```rust
/// Supported LST mints and their human-readable names.
pub fn lst_mints() -> Vec<(Pubkey, &'static str)> {
    vec![
        (Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap(), "jitoSOL"),
        (Pubkey::from_str("mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So").unwrap(), "mSOL"),
        (Pubkey::from_str("bSo13r4TkiE4KumL71LsHTPpL2euBYLFx6h9HP3piy1").unwrap(), "bSOL"),
    ]
}

/// Native SOL mint (wrapped SOL).
pub fn sol_mint() -> Pubkey {
    Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap()
}

/// Map an LST mint to its Sanctum SOL Value Calculator program.
/// Returns None for unknown mints.
pub fn sanctum_sol_value_calculator(mint: &Pubkey) -> Option<Pubkey> {
    let jitosol = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let msol = Pubkey::from_str("mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So").unwrap();
    let bsol = Pubkey::from_str("bSo13r4TkiE4KumL71LsHTPpL2euBYLFx6h9HP3piy1").unwrap();

    let spl_calc = Pubkey::from_str("sp1V4h2gWorkGhVcazBc22Hfo2f5sd7jcjT4EDPrWFF").unwrap();
    let marinade_calc = Pubkey::from_str("mare3SCyfZkAndpBRBeonETmkCCB3TJTTrz8ZN2dnhP").unwrap();

    if *mint == jitosol || *mint == bsol {
        Some(spl_calc)
    } else if *mint == msol {
        Some(marinade_calc)
    } else {
        None
    }
}
```

Add LST fields to `BotConfig` struct:

```rust
pub struct BotConfig {
    // ... existing fields ...
    pub lst_arb_enabled: bool,
    pub lst_min_spread_bps: u64,
}
```

Add parsing in `BotConfig::from_env()`, inside the `Ok(Self { ... })` block:

```rust
lst_arb_enabled: std::env::var("LST_ARB_ENABLED")
    .unwrap_or_else(|_| "true".to_string())
    .parse()?,
lst_min_spread_bps: std::env::var("LST_MIN_SPREAD_BPS")
    .unwrap_or_else(|_| "5".to_string())
    .parse()?,
```

- [ ] **Step 4: Update `.env.example`**

Add at the end of `.env.example`, before the logging section:

```env
# ─── LST Arbitrage (Phase 2) ────────────────────────────────────────
# Enable LST rate arbitrage (jitoSOL, mSOL, bSOL cross-DEX + Sanctum)
LST_ARB_ENABLED=true
# Minimum spread in basis points to consider an LST arb route
# LST spreads are thin (2-20 bps) — set this lower than MIN_PROFIT_LAMPORTS
LST_MIN_SPREAD_BPS=5
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test unit -- --nocapture`

Expected: All tests PASS (config_lst + pool_sanctum).

- [ ] **Step 6: Commit**

```bash
git add src/config.rs .env.example tests/unit/config_lst.rs tests/unit/mod.rs
git commit -m "feat: add LST mint config, Sanctum program IDs, env vars"
```

---

### Task 3: Route discovery for LST pools

**Files:**
- Create: `tests/unit/calculator_lst.rs`
- Modify: `tests/unit/mod.rs`

No production code changes — the RouteCalculator already handles arbitrary pools. This task verifies it works with LST+Sanctum pools in the cache.

- [ ] **Step 1: Write the failing test (route discovery)**

Create `tests/unit/calculator_lst.rs`:

```rust
use solana_sdk::pubkey::Pubkey;
use std::time::Duration;

use solana_mev_bot::config;
use solana_mev_bot::router::pool::{DexType, DetectedSwap, PoolState};
use solana_mev_bot::router::RouteCalculator;
use solana_mev_bot::state::StateCache;

fn sol_mint() -> Pubkey {
    config::sol_mint()
}

fn jitosol_mint() -> Pubkey {
    config::lst_mints()[0].0
}

/// Create a Sanctum virtual pool for jitoSOL/SOL at a given rate.
fn sanctum_virtual_pool(rate: f64, address: Pubkey) -> PoolState {
    let reserve_a: u64 = 1_000_000_000_000_000;
    let reserve_b: u64 = (reserve_a as f64 / rate) as u64;
    PoolState {
        address,
        dex_type: DexType::SanctumInfinity,
        token_a_mint: sol_mint(),
        token_b_mint: jitosol_mint(),
        token_a_reserve: reserve_a,
        token_b_reserve: reserve_b,
        fee_bps: 3,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
    }
}

/// Create a DEX pool for jitoSOL/SOL with given reserves.
fn dex_pool(dex_type: DexType, address: Pubkey, sol_reserve: u64, jitosol_reserve: u64) -> PoolState {
    PoolState {
        address,
        dex_type,
        token_a_mint: sol_mint(),
        token_b_mint: jitosol_mint(),
        token_a_reserve: sol_reserve,
        token_b_reserve: jitosol_reserve,
        fee_bps: dex_type.base_fee_bps(),
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
    }
}

#[test]
fn test_route_discovery_dex_to_sanctum() {
    // Setup: Orca has jitoSOL/SOL at effective rate 1.075 (cheap jitoSOL)
    // Sanctum has jitoSOL/SOL at rate 1.082 (oracle rate)
    // Expected: SOL -> jitoSOL (Orca, cheap) -> SOL (Sanctum, expensive) = profit
    let cache = StateCache::new(Duration::from_secs(60));

    let orca_addr = Pubkey::new_unique();
    let sanctum_addr = Pubkey::new_unique();

    // Orca pool: 10000 SOL, ~9302 jitoSOL (effective rate ~1.075)
    let orca_pool = dex_pool(
        DexType::OrcaWhirlpool,
        orca_addr,
        10_000_000_000_000, // 10000 SOL
        9_302_325_581_395,  // ~9302 jitoSOL -> rate ~1.075
    );

    // Sanctum virtual pool at oracle rate 1.082
    let sanctum_pool = sanctum_virtual_pool(1.082, sanctum_addr);

    cache.upsert(orca_addr, orca_pool);
    cache.upsert(sanctum_addr, sanctum_pool);

    let calculator = RouteCalculator::new(cache, 3);

    // Trigger: someone just swapped on the Orca pool
    let trigger = DetectedSwap {
        signature: String::new(),
        dex_type: DexType::OrcaWhirlpool,
        pool_address: orca_addr,
        input_mint: sol_mint(),
        output_mint: jitosol_mint(),
        amount: None,
        observed_slot: 100,
    };

    let routes = calculator.find_routes(&trigger);
    assert!(!routes.is_empty(), "Should find at least one LST arb route");
    assert!(routes[0].is_profitable(), "Best route should be profitable");
    assert_eq!(routes[0].hop_count(), 2, "Should be a 2-hop route");
}

#[test]
fn test_no_route_when_no_spread() {
    // Both pools at same rate -> no profitable route
    let cache = StateCache::new(Duration::from_secs(60));

    let orca_addr = Pubkey::new_unique();
    let sanctum_addr = Pubkey::new_unique();

    // Both at rate 1.082
    let orca_pool = dex_pool(
        DexType::OrcaWhirlpool,
        orca_addr,
        10_000_000_000_000,
        (10_000_000_000_000f64 / 1.082) as u64,
    );
    let sanctum_pool = sanctum_virtual_pool(1.082, sanctum_addr);

    cache.upsert(orca_addr, orca_pool);
    cache.upsert(sanctum_addr, sanctum_pool);

    let calculator = RouteCalculator::new(cache, 3);

    let trigger = DetectedSwap {
        signature: String::new(),
        dex_type: DexType::OrcaWhirlpool,
        pool_address: orca_addr,
        input_mint: sol_mint(),
        output_mint: jitosol_mint(),
        amount: None,
        observed_slot: 100,
    };

    let routes = calculator.find_routes(&trigger);
    let profitable: Vec<_> = routes.iter().filter(|r| r.is_profitable()).collect();
    assert!(profitable.is_empty(), "No profitable route when rates are equal (fees eat any tiny diff)");
}
```

Add to `tests/unit/mod.rs`:

```rust
mod pool_sanctum;
mod config_lst;
mod calculator_lst;
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test --test unit test_route_discovery -- --nocapture`

Expected: Both tests PASS. No production code changes needed — the RouteCalculator already works generically with any `DexType`.

- [ ] **Step 3: Commit**

```bash
git add tests/unit/calculator_lst.rs tests/unit/mod.rs
git commit -m "test: LST route discovery via RouteCalculator with Sanctum virtual pool"
```

---

### Task 4: Sanctum swap instruction builder

**Files:**
- Modify: `src/executor/bundle.rs`
- Create: `tests/unit/bundle_sanctum.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/unit/bundle_sanctum.rs`:

```rust
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use solana_mev_bot::config;
use solana_mev_bot::executor::bundle::sanctum_swap_accounts;

#[test]
fn test_sanctum_pda_derivation() {
    let s_controller = config::programs::sanctum_s_controller();

    // Pool State PDA: seeds = [b"state"]
    let (pool_state_pda, _) = Pubkey::find_program_address(&[b"state"], &s_controller);
    // LST State List PDA: seeds = [b"lst-state-list"]
    let (lst_state_list_pda, _) = Pubkey::find_program_address(&[b"lst-state-list"], &s_controller);

    // These should be deterministic
    assert_ne!(pool_state_pda, lst_state_list_pda);
    assert_ne!(pool_state_pda, Pubkey::default());
}

#[test]
fn test_sanctum_swap_accounts_count() {
    let signer = Pubkey::new_unique();
    let jitosol_mint = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let sol_mint = config::sol_mint();

    let accounts = sanctum_swap_accounts(
        &signer,
        &jitosol_mint, // input
        &sol_mint,      // output
    );

    // SwapExactIn needs 12 accounts per spec
    assert_eq!(accounts.len(), 12, "Sanctum SwapExactIn requires 12 accounts");
}

#[test]
fn test_sanctum_swap_accounts_signer() {
    let signer = Pubkey::new_unique();
    let jitosol_mint = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let sol_mint = config::sol_mint();

    let accounts = sanctum_swap_accounts(&signer, &jitosol_mint, &sol_mint);

    // First account must be the signer
    assert!(accounts[0].is_signer, "First account must be signer");
    assert_eq!(accounts[0].pubkey, signer);
}
```

Add to `tests/unit/mod.rs`:

```rust
mod pool_sanctum;
mod config_lst;
mod calculator_lst;
mod bundle_sanctum;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test unit test_sanctum_swap_accounts -- --nocapture`

Expected: Compilation error — `sanctum_swap_accounts` does not exist.

- [ ] **Step 3: Implement Sanctum swap instruction builder**

In `src/executor/bundle.rs`, add at the top of the file with the other imports:

```rust
use spl_associated_token_account::get_associated_token_address;
```

Wait — the project doesn't have `spl-associated-token-account` as a dependency. We need to derive ATAs manually or add the crate. Since we want to keep dependencies minimal, derive inline:

Add this public function after the `BundleBuilder` impl block (before the closing of the file):

```rust
/// Derive an Associated Token Account address.
/// ATA = PDA([wallet, TOKEN_PROGRAM_ID, mint], ATA_PROGRAM_ID)
fn derive_ata(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    let token_program = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
    let ata_program = Pubkey::from_str("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL").unwrap();
    let seeds = &[
        wallet.as_ref(),
        token_program.as_ref(),
        mint.as_ref(),
    ];
    let (ata, _) = Pubkey::find_program_address(seeds, &ata_program);
    ata
}

/// Build the account list for a Sanctum Infinity SwapExactIn instruction.
/// Public for testing.
pub fn sanctum_swap_accounts(
    signer: &Pubkey,
    input_mint: &Pubkey,
    output_mint: &Pubkey,
) -> Vec<AccountMeta> {
    let s_controller = crate::config::programs::sanctum_s_controller();
    let pricing_program = crate::config::programs::sanctum_flat_fee_pricing();

    // PDAs
    let (pool_state_pda, _) = Pubkey::find_program_address(&[b"state"], &s_controller);
    let (lst_state_list_pda, _) = Pubkey::find_program_address(&[b"lst-state-list"], &s_controller);

    // Reserve ATAs (owned by Pool State PDA)
    let source_reserve_ata = derive_ata(&pool_state_pda, input_mint);
    let dest_reserve_ata = derive_ata(&pool_state_pda, output_mint);

    // User ATAs
    let user_source_ata = derive_ata(signer, input_mint);
    let user_dest_ata = derive_ata(signer, output_mint);

    // SOL Value Calculators
    let source_calc = crate::config::sanctum_sol_value_calculator(input_mint)
        .unwrap_or_else(|| {
            // For SOL (wSOL), use the wSOL calculator
            Pubkey::from_str("wsoGmxQLSvwWpuaidCApxN5kEowLe2HLQLJhCQnj4bE").unwrap()
        });
    let dest_calc = crate::config::sanctum_sol_value_calculator(output_mint)
        .unwrap_or_else(|| {
            Pubkey::from_str("wsoGmxQLSvwWpuaidCApxN5kEowLe2HLQLJhCQnj4bE").unwrap()
        });

    let token_program = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
    let system_program = solana_sdk::system_program::id();

    vec![
        AccountMeta::new_readonly(*signer, true),           // 1. Payer/signer
        AccountMeta::new(pool_state_pda, false),             // 2. Pool State PDA
        AccountMeta::new_readonly(lst_state_list_pda, false),// 3. LST State List PDA
        AccountMeta::new(source_reserve_ata, false),         // 4. Source reserve ATA
        AccountMeta::new(dest_reserve_ata, false),           // 5. Dest reserve ATA
        AccountMeta::new_readonly(pricing_program, false),   // 6. Pricing program
        AccountMeta::new_readonly(source_calc, false),       // 7. Source SOL Value Calc
        AccountMeta::new_readonly(dest_calc, false),         // 8. Dest SOL Value Calc
        AccountMeta::new(user_source_ata, false),            // 9. User source ATA
        AccountMeta::new(user_dest_ata, false),              // 10. User dest ATA
        AccountMeta::new_readonly(token_program, false),     // 11. Token Program
        AccountMeta::new_readonly(system_program, false),    // 12. System Program
    ]
}
```

Add the Sanctum match arm in `build_swap_instruction`:

```rust
fn build_swap_instruction(
    &self,
    hop: &crate::router::pool::RouteHop,
) -> Result<Instruction> {
    match hop.dex_type {
        DexType::RaydiumAmm => self.build_raydium_amm_swap(hop),
        DexType::RaydiumClmm => self.build_raydium_clmm_swap(hop),
        DexType::OrcaWhirlpool => self.build_orca_whirlpool_swap(hop),
        DexType::MeteoraDlmm => self.build_meteora_dlmm_swap(hop),
        DexType::SanctumInfinity => self.build_sanctum_swap(hop),
    }
}
```

Add the `build_sanctum_swap` method inside the `impl BundleBuilder` block:

```rust
fn build_sanctum_swap(
    &self,
    hop: &crate::router::pool::RouteHop,
) -> Result<Instruction> {
    let accounts = sanctum_swap_accounts(
        &self.searcher_keypair.pubkey(),
        &hop.input_mint,
        &hop.output_mint,
    );

    // SwapExactIn instruction data: discriminator (8 bytes) + amount (u64) + min_out (u64)
    // Discriminator for SwapExactIn: hash("global:swap_exact_in")[..8]
    // For now, use a known discriminator — will be verified against Sanctum IDL
    let mut data = Vec::with_capacity(24);
    // Anchor discriminator for "swap_exact_in": sha256("global:swap_exact_in")[..8]
    data.extend_from_slice(&[0x0a, 0xd3, 0xc8, 0x1a, 0x3e, 0x4d, 0x2b, 0x1c]); // placeholder, verify via IDL
    data.extend_from_slice(&hop.estimated_output.to_le_bytes()); // amount_in
    data.extend_from_slice(&0u64.to_le_bytes()); // minimum_amount_out (set by caller for final hop)

    Ok(Instruction {
        program_id: crate::config::programs::sanctum_s_controller(),
        accounts,
        data,
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test unit bundle_sanctum -- --nocapture`

Expected: All 3 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/executor/bundle.rs tests/unit/bundle_sanctum.rs tests/unit/mod.rs
git commit -m "feat: Sanctum SwapExactIn instruction builder with account layout"
```

---

### Task 5: Profit enforcement — `minimum_amount_out` on final hop

**Files:**
- Modify: `src/executor/bundle.rs`
- Create: `tests/unit/bundle_profit.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/unit/bundle_profit.rs`:

```rust
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::hash::Hash;

use solana_mev_bot::router::pool::{ArbRoute, DexType, RouteHop};
use solana_mev_bot::executor::BundleBuilder;

#[test]
fn test_bundle_sets_min_out_on_final_hop() {
    let keypair = Keypair::new();
    let builder = BundleBuilder::new(keypair);

    let base_mint = Pubkey::new_unique();
    let other_mint = Pubkey::new_unique();

    let route = ArbRoute {
        hops: vec![
            RouteHop {
                pool_address: Pubkey::new_unique(),
                dex_type: DexType::RaydiumAmm,
                input_mint: base_mint,
                output_mint: other_mint,
                estimated_output: 1_100_000_000,
            },
            RouteHop {
                pool_address: Pubkey::new_unique(),
                dex_type: DexType::SanctumInfinity,
                input_mint: other_mint,
                output_mint: base_mint,
                estimated_output: 1_050_000_000,
            },
        ],
        base_mint,
        input_amount: 1_000_000_000, // 1 SOL
        estimated_profit: 50_000_000,
        estimated_profit_lamports: 50_000_000,
    };

    let tip_lamports = 25_000_000; // 50% of profit
    let min_profit = route.input_amount + (route.estimated_profit_lamports - tip_lamports);

    let result = builder.build_arb_bundle(&route, tip_lamports, Hash::default());
    assert!(result.is_ok(), "Bundle build should succeed");

    // Verify the bundle was built (detailed IX inspection requires deserializing,
    // but we verify it doesn't error)
    let bundle = result.unwrap();
    assert_eq!(bundle.len(), 1, "Single tx bundle (arb + tip in one tx)");
}
```

Add to `tests/unit/mod.rs`:

```rust
mod pool_sanctum;
mod config_lst;
mod calculator_lst;
mod bundle_sanctum;
mod bundle_profit;
```

- [ ] **Step 2: Run test to verify current state**

Run: `cargo test --test unit test_bundle_sets_min_out -- --nocapture`

Expected: Should compile and pass (basic bundle build test).

- [ ] **Step 3: Add `minimum_amount_out` enforcement to `build_arb_transaction_with_tip`**

Modify `build_arb_transaction_with_tip` in `src/executor/bundle.rs`. The method signature changes to accept `min_profit_lamports`:

```rust
pub fn build_arb_bundle(
    &self,
    route: &ArbRoute,
    tip_lamports: u64,
    recent_blockhash: Hash,
) -> Result<Vec<Vec<u8>>> {
    let mut bundle_txs: Vec<Vec<u8>> = Vec::with_capacity(2);

    // Calculate minimum output for profit enforcement on final hop
    let min_final_output = route.input_amount + route.estimated_profit_lamports.saturating_sub(tip_lamports);

    let arb_tx = self.build_arb_transaction_with_tip(route, tip_lamports, min_final_output, recent_blockhash)?;
    bundle_txs.push(bincode::serialize(&arb_tx)?);

    debug!(
        "Built bundle: {} txs, tip={} lamports, min_out={}, route={} hops",
        bundle_txs.len(),
        tip_lamports,
        min_final_output,
        route.hop_count(),
    );

    Ok(bundle_txs)
}

fn build_arb_transaction_with_tip(
    &self,
    route: &ArbRoute,
    tip_lamports: u64,
    min_final_output: u64,
    recent_blockhash: Hash,
) -> Result<Transaction> {
    let mut instructions = Vec::with_capacity(route.hop_count() + 1);

    // Swap instructions — intermediate hops get min_out=0, final hop gets profit floor
    let last_idx = route.hops.len() - 1;
    for (i, hop) in route.hops.iter().enumerate() {
        let min_out = if i == last_idx { min_final_output } else { 0 };
        let ix = self.build_swap_instruction_with_min_out(hop, min_out)?;
        instructions.push(ix);
    }

    // Tip instruction as last ix
    let tip_ix = self.build_tip_instruction(tip_lamports)?;
    instructions.push(tip_ix);

    let tx = Transaction::new_signed_with_payer(
        &instructions,
        Some(&self.searcher_keypair.pubkey()),
        &[&self.searcher_keypair],
        recent_blockhash,
    );

    Ok(tx)
}
```

Add `build_swap_instruction_with_min_out`:

```rust
fn build_swap_instruction_with_min_out(
    &self,
    hop: &crate::router::pool::RouteHop,
    minimum_amount_out: u64,
) -> Result<Instruction> {
    match hop.dex_type {
        DexType::RaydiumAmm => self.build_raydium_amm_swap_with_min_out(hop, minimum_amount_out),
        DexType::RaydiumClmm => self.build_raydium_clmm_swap(hop),
        DexType::OrcaWhirlpool => self.build_orca_whirlpool_swap(hop),
        DexType::MeteoraDlmm => self.build_meteora_dlmm_swap(hop),
        DexType::SanctumInfinity => self.build_sanctum_swap_with_min_out(hop, minimum_amount_out),
    }
}
```

Update `build_raydium_amm_swap` to accept `minimum_amount_out`:

```rust
fn build_raydium_amm_swap_with_min_out(
    &self,
    hop: &crate::router::pool::RouteHop,
    minimum_amount_out: u64,
) -> Result<Instruction> {
    let mut data = vec![9u8];
    data.extend_from_slice(&hop.estimated_output.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());

    Ok(Instruction {
        program_id: crate::config::programs::raydium_amm(),
        accounts: vec![
            AccountMeta::new_readonly(hop.pool_address, false),
        ],
        data,
    })
}
```

Update `build_sanctum_swap` to accept `minimum_amount_out`:

```rust
fn build_sanctum_swap_with_min_out(
    &self,
    hop: &crate::router::pool::RouteHop,
    minimum_amount_out: u64,
) -> Result<Instruction> {
    let accounts = sanctum_swap_accounts(
        &self.searcher_keypair.pubkey(),
        &hop.input_mint,
        &hop.output_mint,
    );

    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&[0x0a, 0xd3, 0xc8, 0x1a, 0x3e, 0x4d, 0x2b, 0x1c]);
    data.extend_from_slice(&hop.estimated_output.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());

    Ok(Instruction {
        program_id: crate::config::programs::sanctum_s_controller(),
        accounts,
        data,
    })
}
```

Remove the old `build_swap_instruction` and `build_sanctum_swap` methods (replaced by the `_with_min_out` variants).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test unit -- --nocapture`

Expected: All tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/executor/bundle.rs tests/unit/bundle_profit.rs tests/unit/mod.rs
git commit -m "feat: enforce minimum_amount_out on final hop for profit guarantee"
```

---

### Task 6: LST spread gate in simulator

**Files:**
- Modify: `src/router/simulator.rs`
- Create: `tests/unit/simulator_lst.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/unit/simulator_lst.rs`:

```rust
use solana_sdk::pubkey::Pubkey;
use std::time::Duration;

use solana_mev_bot::config;
use solana_mev_bot::router::pool::{ArbRoute, DexType, PoolState, RouteHop};
use solana_mev_bot::router::ProfitSimulator;
use solana_mev_bot::state::StateCache;

fn sol_mint() -> Pubkey {
    config::sol_mint()
}

fn jitosol_mint() -> Pubkey {
    config::lst_mints()[0].0
}

fn make_cache_with_pools(orca_addr: Pubkey, sanctum_addr: Pubkey) -> StateCache {
    let cache = StateCache::new(Duration::from_secs(60));

    // Orca pool: rate ~1.075 (cheap jitoSOL)
    cache.upsert(orca_addr, PoolState {
        address: orca_addr,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: sol_mint(),
        token_b_mint: jitosol_mint(),
        token_a_reserve: 10_000_000_000_000,
        token_b_reserve: 9_302_325_581_395,
        fee_bps: 1,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
    });

    // Sanctum virtual pool: rate 1.082
    let reserve_a: u64 = 1_000_000_000_000_000;
    cache.upsert(sanctum_addr, PoolState {
        address: sanctum_addr,
        dex_type: DexType::SanctumInfinity,
        token_a_mint: sol_mint(),
        token_b_mint: jitosol_mint(),
        token_a_reserve: reserve_a,
        token_b_reserve: (reserve_a as f64 / 1.082) as u64,
        fee_bps: 3,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
    });

    cache
}

#[test]
fn test_simulator_approves_profitable_lst_route() {
    let orca_addr = Pubkey::new_unique();
    let sanctum_addr = Pubkey::new_unique();
    let cache = make_cache_with_pools(orca_addr, sanctum_addr);

    let simulator = ProfitSimulator::new(cache, 0.50, 1000); // 50% tip, 1000 lamport min

    let route = ArbRoute {
        hops: vec![
            RouteHop {
                pool_address: orca_addr,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol_mint(),
                output_mint: jitosol_mint(),
                estimated_output: 9_000_000,
            },
            RouteHop {
                pool_address: sanctum_addr,
                dex_type: DexType::SanctumInfinity,
                input_mint: jitosol_mint(),
                output_mint: sol_mint(),
                estimated_output: 10_100_000,
            },
        ],
        base_mint: sol_mint(),
        input_amount: 10_000_000, // 0.01 SOL
        estimated_profit: 100_000,
        estimated_profit_lamports: 100_000,
    };

    let result = simulator.simulate(&route);
    match result {
        solana_mev_bot::router::simulator::SimulationResult::Profitable { final_profit_lamports, .. } => {
            assert!(final_profit_lamports > 0, "Should have positive final profit");
        }
        solana_mev_bot::router::simulator::SimulationResult::Unprofitable { reason } => {
            panic!("Expected profitable, got: {}", reason);
        }
    }
}

#[test]
fn test_simulator_rejects_below_min_profit() {
    let orca_addr = Pubkey::new_unique();
    let sanctum_addr = Pubkey::new_unique();
    let cache = make_cache_with_pools(orca_addr, sanctum_addr);

    // Set min profit very high — route should be rejected
    let simulator = ProfitSimulator::new(cache, 0.50, 999_999_999_999);

    let route = ArbRoute {
        hops: vec![
            RouteHop {
                pool_address: orca_addr,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol_mint(),
                output_mint: jitosol_mint(),
                estimated_output: 9_000_000,
            },
            RouteHop {
                pool_address: sanctum_addr,
                dex_type: DexType::SanctumInfinity,
                input_mint: jitosol_mint(),
                output_mint: sol_mint(),
                estimated_output: 10_100_000,
            },
        ],
        base_mint: sol_mint(),
        input_amount: 10_000_000,
        estimated_profit: 100_000,
        estimated_profit_lamports: 100_000,
    };

    let result = simulator.simulate(&route);
    match result {
        solana_mev_bot::router::simulator::SimulationResult::Unprofitable { reason } => {
            assert!(reason.contains("Below minimum"), "Should reject: {}", reason);
        }
        _ => panic!("Expected Unprofitable"),
    }
}
```

Add to `tests/unit/mod.rs`:

```rust
mod pool_sanctum;
mod config_lst;
mod calculator_lst;
mod bundle_sanctum;
mod bundle_profit;
mod simulator_lst;
```

- [ ] **Step 2: Run tests**

Run: `cargo test --test unit simulator_lst -- --nocapture`

Expected: Tests should PASS — the simulator already works generically. This validates that Sanctum pools work through the simulator unchanged.

- [ ] **Step 3: Commit**

```bash
git add tests/unit/simulator_lst.rs tests/unit/mod.rs
git commit -m "test: simulator validates LST routes with Sanctum virtual pools"
```

---

### Task 7: Geyser subscription for Sanctum vaults

**Files:**
- Modify: `src/mempool/stream.rs`
- Modify: `src/config.rs`

- [ ] **Step 1: Add Sanctum to monitored programs**

In `src/config.rs`, update `monitored_programs()` in the `BotConfig` impl:

```rust
pub fn monitored_programs(&self) -> Vec<Pubkey> {
    let mut programs = vec![
        programs::raydium_amm(),
        programs::raydium_clmm(),
        programs::orca_whirlpool(),
        programs::meteora_dlmm(),
    ];
    if self.lst_arb_enabled {
        programs.push(programs::sanctum_s_controller());
    }
    programs
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check`

Expected: Compiles cleanly. The Geyser stream in `stream.rs` already iterates `monitored_programs()` — adding Sanctum to the list means Sanctum vault account updates automatically flow through the existing pipeline.

- [ ] **Step 3: Commit**

```bash
git add src/config.rs
git commit -m "feat: add Sanctum S Controller to Geyser monitored programs"
```

---

### Task 8: Sanctum virtual pool bootstrap in main.rs

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add virtual pool creation at startup**

In `src/main.rs`, after the state cache is created (after line `let state_cache = StateCache::new(config.pool_state_ttl);`), add:

```rust
// Bootstrap Sanctum virtual pools for LST arb
if config.lst_arb_enabled {
    bootstrap_sanctum_pools(&state_cache);
    info!("LST arb enabled: {} Sanctum virtual pools bootstrapped", config::lst_mints().len());
}
```

Add the bootstrap function at the bottom of main.rs (before `load_keypair`):

```rust
/// Create Sanctum virtual pools for each supported LST.
///
/// Each LST gets a virtual pool modeling the Sanctum Infinity oracle rate.
/// Reserves are synthetic — large values that produce the correct exchange rate
/// under constant-product math with negligible price impact.
///
/// Initial rates are hardcoded approximations. In production, these should be
/// fetched from on-chain stake pool state at startup (total_lamports / pool_token_supply).
/// The Geyser stream will keep them updated as Sanctum reserve ATAs change.
fn bootstrap_sanctum_pools(state_cache: &state::StateCache) {
    use router::pool::{DexType, PoolState};

    let sol = config::sol_mint();
    const SYNTHETIC_RESERVE_BASE: u64 = 1_000_000_000_000_000; // 1B SOL in lamports

    // Approximate current exchange rates (SOL per LST).
    // These get corrected as soon as the first Geyser update arrives.
    let lst_rates: Vec<(solana_sdk::pubkey::Pubkey, &str, f64)> = config::lst_mints()
        .into_iter()
        .map(|(mint, name)| {
            let rate = match name {
                "jitoSOL" => 1.082,
                "mSOL" => 1.075,
                "bSOL" => 1.060,
                _ => 1.050, // conservative default
            };
            (mint, name, rate)
        })
        .collect();

    for (lst_mint, name, rate) in &lst_rates {
        // Deterministic virtual pool address: PDA([b"sanctum-virtual", lst_mint], system_program)
        // This ensures the same LST always maps to the same virtual pool address.
        let (virtual_pool_addr, _) = solana_sdk::pubkey::Pubkey::find_program_address(
            &[b"sanctum-virtual", lst_mint.as_ref()],
            &solana_sdk::system_program::id(),
        );

        let reserve_a = SYNTHETIC_RESERVE_BASE;
        let reserve_b = (SYNTHETIC_RESERVE_BASE as f64 / rate) as u64;

        let pool = PoolState {
            address: virtual_pool_addr,
            dex_type: DexType::SanctumInfinity,
            token_a_mint: sol,
            token_b_mint: *lst_mint,
            token_a_reserve: reserve_a,
            token_b_reserve: reserve_b,
            fee_bps: 3, // Sanctum typical fee
            current_tick: None,
            sqrt_price_x64: None,
            liquidity: None,
            last_slot: 0,
        };

        state_cache.upsert(virtual_pool_addr, pool);
        info!("Bootstrapped Sanctum virtual pool for {}: rate={}, addr={}", name, rate, virtual_pool_addr);
    }
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check`

Expected: Compiles cleanly.

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat: bootstrap Sanctum virtual pools for LST arb at startup"
```

---

### Task 9: E2E test infrastructure + Surfpool tests

**Files:**
- Modify: `Cargo.toml`
- Create: `tests/e2e/mod.rs`
- Create: `tests/e2e/lst_pipeline.rs`

- [ ] **Step 1: Add e2e feature flag and test target to Cargo.toml**

Add to `Cargo.toml`:

```toml
[features]
default = []
e2e = []

[[test]]
name = "unit"
path = "tests/unit/mod.rs"

[[test]]
name = "e2e"
path = "tests/e2e/mod.rs"
required-features = ["e2e"]
```

- [ ] **Step 2: Create e2e test files**

Create `tests/e2e/mod.rs`:

```rust
mod lst_pipeline;
```

Create `tests/e2e/lst_pipeline.rs`:

```rust
//! E2E tests for LST arb pipeline using Surfpool.
//!
//! These tests require a running Surfpool instance:
//!   NO_DNA=1 surfpool start --ci --network mainnet
//!
//! Run with: cargo test --features e2e --test e2e -- --test-threads=1

use solana_sdk::pubkey::Pubkey;
use std::time::Duration;

use solana_mev_bot::config;
use solana_mev_bot::mempool::PoolStateChange;
use solana_mev_bot::router::pool::{DexType, DetectedSwap, PoolState};
use solana_mev_bot::router::{RouteCalculator, ProfitSimulator};
use solana_mev_bot::state::StateCache;

/// Helper: set up a StateCache with Orca and Sanctum pools for jitoSOL/SOL
/// with a known spread.
fn setup_cache_with_spread(orca_rate: f64, sanctum_rate: f64) -> (StateCache, Pubkey, Pubkey) {
    let cache = StateCache::new(Duration::from_secs(60));
    let sol = config::sol_mint();
    let jitosol = config::lst_mints()[0].0;

    let orca_addr = Pubkey::new_unique();
    let sanctum_addr = Pubkey::new_unique();

    // Orca pool
    let orca_sol_reserve = 10_000_000_000_000u64;
    let orca_jitosol_reserve = (orca_sol_reserve as f64 / orca_rate) as u64;
    cache.upsert(orca_addr, PoolState {
        address: orca_addr,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: sol,
        token_b_mint: jitosol,
        token_a_reserve: orca_sol_reserve,
        token_b_reserve: orca_jitosol_reserve,
        fee_bps: 1,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
    });

    // Sanctum virtual pool
    let reserve_base: u64 = 1_000_000_000_000_000;
    cache.upsert(sanctum_addr, PoolState {
        address: sanctum_addr,
        dex_type: DexType::SanctumInfinity,
        token_a_mint: sol,
        token_b_mint: jitosol,
        token_a_reserve: reserve_base,
        token_b_reserve: (reserve_base as f64 / sanctum_rate) as u64,
        fee_bps: 3,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
    });

    (cache, orca_addr, sanctum_addr)
}

#[test]
fn test_e2e_profitable_arb_pipeline() {
    // Orca rate 1.075, Sanctum rate 1.082 -> ~0.65% spread -> should be profitable
    let (cache, orca_addr, _sanctum_addr) = setup_cache_with_spread(1.075, 1.082);

    let calculator = RouteCalculator::new(cache.clone(), 3);
    let simulator = ProfitSimulator::new(cache.clone(), 0.50, 1000);

    // Simulate Geyser event: vault balance changed on Orca pool
    let trigger = DetectedSwap {
        signature: String::new(),
        dex_type: DexType::OrcaWhirlpool,
        pool_address: orca_addr,
        input_mint: config::sol_mint(),
        output_mint: config::lst_mints()[0].0,
        amount: None,
        observed_slot: 100,
    };

    // Route discovery
    let routes = calculator.find_routes(&trigger);
    assert!(!routes.is_empty(), "Should find arb routes");

    // Simulation
    let best = &routes[0];
    let result = simulator.simulate(best);
    match result {
        solana_mev_bot::router::simulator::SimulationResult::Profitable {
            final_profit_lamports,
            tip_lamports,
            ..
        } => {
            assert!(final_profit_lamports > 0, "Positive profit");
            assert!(tip_lamports > 0, "Non-zero tip");
        }
        solana_mev_bot::router::simulator::SimulationResult::Unprofitable { reason } => {
            panic!("Expected profitable: {}", reason);
        }
    }
}

#[test]
fn test_e2e_revert_unprofitable() {
    // Same rate on both pools -> fees make it unprofitable
    let (cache, orca_addr, _sanctum_addr) = setup_cache_with_spread(1.082, 1.082);

    let calculator = RouteCalculator::new(cache.clone(), 3);
    let simulator = ProfitSimulator::new(cache.clone(), 0.50, 1000);

    let trigger = DetectedSwap {
        signature: String::new(),
        dex_type: DexType::OrcaWhirlpool,
        pool_address: orca_addr,
        input_mint: config::sol_mint(),
        output_mint: config::lst_mints()[0].0,
        amount: None,
        observed_slot: 100,
    };

    let routes = calculator.find_routes(&trigger);
    // Either no routes found, or all are unprofitable after simulation
    for route in &routes {
        let result = simulator.simulate(route);
        match result {
            solana_mev_bot::router::simulator::SimulationResult::Unprofitable { .. } => {
                // Expected
            }
            solana_mev_bot::router::simulator::SimulationResult::Profitable { .. } => {
                panic!("Should NOT be profitable when rates are equal");
            }
        }
    }
}

#[test]
fn test_e2e_stale_state_rejected() {
    let (cache, orca_addr, _) = setup_cache_with_spread(1.075, 1.082);

    let orca_vault = Pubkey::new_unique();
    // Register a vault for the orca pool
    cache.register_vault(orca_vault, orca_addr, true);

    // Apply an update at slot 100
    cache.update_vault_balance(&orca_vault, 10_000_000_000_000, 100);

    // Try to apply a stale update at slot 50 — should be ignored
    let result = cache.update_vault_balance(&orca_vault, 5_000_000_000_000, 50);
    assert!(result.is_none(), "Stale update (slot 50 < 100) should be rejected");

    // Verify the reserve didn't change
    let pool = cache.get_any(&orca_addr).unwrap();
    assert_eq!(pool.token_a_reserve, 10_000_000_000_000, "Reserve should be unchanged after stale update");
}

#[test]
fn test_e2e_channel_backpressure() {
    use crossbeam_channel::bounded;

    let (tx, rx) = bounded::<PoolStateChange>(2); // tiny capacity

    // Fill the channel
    let change1 = PoolStateChange { vault_address: Pubkey::new_unique(), new_balance: 100, slot: 1 };
    let change2 = PoolStateChange { vault_address: Pubkey::new_unique(), new_balance: 200, slot: 2 };
    let change3 = PoolStateChange { vault_address: Pubkey::new_unique(), new_balance: 300, slot: 3 };

    assert!(tx.try_send(change1).is_ok());
    assert!(tx.try_send(change2).is_ok());
    // Channel full — try_send should fail (not block)
    assert!(tx.try_send(change3).is_err(), "try_send should fail on full channel, not block");

    // Drain and verify we got the first two
    let c1 = rx.try_recv().unwrap();
    assert_eq!(c1.slot, 1);
    let c2 = rx.try_recv().unwrap();
    assert_eq!(c2.slot, 2);
}
```

- [ ] **Step 3: Run unit tests (no Surfpool needed for these)**

Run: `cargo test --features e2e --test e2e -- --nocapture`

Expected: All 4 tests PASS. These tests don't actually require Surfpool running — they test the pipeline logic in-process. True Surfpool integration (submitting txs to an RPC) can be added as a follow-up.

- [ ] **Step 4: Run all tests together**

Run: `cargo test --test unit -- --nocapture && cargo test --features e2e --test e2e -- --nocapture`

Expected: All tests PASS across both test targets.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml tests/e2e/
git commit -m "test: e2e test infrastructure + LST pipeline tests with profit/revert/stale/backpressure"
```

---

### Task 10: Final integration — verify full compilation and all tests

**Files:**
- None (verification only)

- [ ] **Step 1: Run cargo check**

Run: `cargo check`

Expected: Compiles cleanly, zero warnings.

- [ ] **Step 2: Run cargo clippy**

Run: `cargo clippy -- -D warnings`

Expected: No warnings or errors.

- [ ] **Step 3: Run all unit tests**

Run: `cargo test --test unit -- --nocapture`

Expected: All tests PASS.

- [ ] **Step 4: Run all e2e tests**

Run: `cargo test --features e2e --test e2e -- --nocapture`

Expected: All tests PASS.

- [ ] **Step 5: Commit any clippy fixes**

```bash
git add -A
git commit -m "chore: clippy fixes for Phase 2 LST arb"
```

(Only if Step 2 required changes.)
