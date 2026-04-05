# Arb-Guard Passthrough CPI Client Integration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Update the client-side bundle builder to use the new `execute_arb_v2` passthrough CPI instruction for ALL DEX routes, replacing the Orca-only `execute_arb` path and the `start_check`/`profit_check` wrapping.

**Architecture:** The on-chain `execute_arb_v2` is already deployed. The client builds per-hop swap IXs using existing builders, decomposes each into `(program_id, accounts, data)`, packs them into `HopV2Params`, and emits a single `execute_arb_v2` instruction with all accounts in `remaining_accounts`. The old Orca-only CPI path and the start_check/profit_check wrapping path are removed.

**Tech Stack:** Rust, solana-sdk 4.0, Anchor Borsh serialization (for instruction data)

---

## File Structure

| File | Responsibility | Change |
|------|---------------|--------|
| `src/executor/bundle.rs` | MODIFY | Replace `build_arb_instructions` to always use `execute_arb_v2`. Replace `build_execute_arb_ix` with `build_execute_arb_v2_ix` that works with all DEXes. Remove old Orca-only CPI path and start_check/profit_check insertion. |
| `tests/unit/arb_guard_cpi.rs` | MODIFY | TDD: new tests for `execute_arb_v2` IX builder with multi-DEX routes. Update/remove old Orca-only tests. |
| `tests/unit/bundle_profit.rs` | MODIFY | May need updates if test relies on start_check/profit_check wrapping. |

---

### Task 1: Write failing tests for execute_arb_v2 IX builder

**Files:**
- Modify: `tests/unit/arb_guard_cpi.rs`

- [ ] **Step 1: Write failing tests**

Add these tests to `tests/unit/arb_guard_cpi.rs`:

```rust
use std::time::Duration;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_mev_bot::config;
use solana_mev_bot::executor::BundleBuilder;
use solana_mev_bot::router::pool::{ArbRoute, DexType, PoolExtra, PoolState, RouteHop};
use solana_mev_bot::state::StateCache;

/// Helper: create a cache with two CPMM pools for SOL→TOKEN→SOL
fn setup_two_pool_cache() -> (StateCache, Pubkey, Pubkey, Pubkey) {
    let cache = StateCache::new(Duration::from_secs(60));
    let sol = config::sol_mint();
    let token = Pubkey::new_unique();

    let pool_a = Pubkey::new_unique();
    cache.upsert(pool_a, PoolState {
        address: pool_a,
        dex_type: DexType::OrcaWhirlpool,
        token_a_mint: sol,
        token_b_mint: token,
        token_a_reserve: 10_000_000_000_000,
        token_b_reserve: 10_000_000_000_000,
        fee_bps: 25,
        current_tick: Some(0),
        sqrt_price_x64: Some(1u128 << 64),
        liquidity: Some(1_000_000_000),
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            tick_spacing: Some(64),
            observation: Some(Pubkey::new_unique()),
            token_program_a: None,
            token_program_b: None,
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    });

    let pool_b = Pubkey::new_unique();
    cache.upsert(pool_b, PoolState {
        address: pool_b,
        dex_type: DexType::RaydiumCp,
        token_a_mint: sol,
        token_b_mint: token,
        token_a_reserve: 10_000_000_000_000,
        token_b_reserve: 10_050_000_000_000,
        fee_bps: 25,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            token_program_a: Some(Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")),
            token_program_b: Some(Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")),
            ..Default::default()
        },
        best_bid_price: None,
        best_ask_price: None,
    });

    (cache, pool_a, pool_b, token)
}

/// With arb-guard configured, build_arb_instructions should produce a single
/// execute_arb_v2 IX (via CPI) for any DEX combination, not just Orca.
#[test]
fn test_execute_arb_v2_multi_dex_produces_single_ix() {
    let (cache, pool_a, pool_b, token) = setup_two_pool_cache();
    let sol = config::sol_mint();
    let guard_id = Pubkey::new_unique();
    let builder = BundleBuilder::new(Keypair::new(), cache, Some(guard_id));

    let route = ArbRoute {
        base_mint: sol,
        input_amount: 1_000_000,
        estimated_profit: 50_000,
        estimated_profit_lamports: 50_000,
        hops: vec![
            RouteHop {
                pool_address: pool_a,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol,
                output_mint: token,
                estimated_output: 1_000_000,
            },
            RouteHop {
                pool_address: pool_b,
                dex_type: DexType::RaydiumCp,
                input_mint: token,
                output_mint: sol,
                estimated_output: 1_050_000,
            },
        ],
    };

    let ixs = builder.build_arb_instructions(&route, 1_000_000).unwrap();

    // Should have: compute budget + ATA creates + wSOL wrap + execute_arb_v2 + wSOL unwrap
    // The execute_arb_v2 IX should be the one with the arb-guard program_id
    let arb_ix = ixs.iter().find(|ix| ix.program_id == guard_id);
    assert!(arb_ix.is_some(), "Should contain an execute_arb_v2 instruction with guard program");

    // Should NOT contain start_check or profit_check as separate instructions
    let guard_ixs: Vec<_> = ixs.iter().filter(|ix| ix.program_id == guard_id).collect();
    assert_eq!(guard_ixs.len(), 1, "Should have exactly 1 guard IX (execute_arb_v2), not start_check + profit_check");
}

/// execute_arb_v2 remaining_accounts should contain all accounts from both hops
#[test]
fn test_execute_arb_v2_remaining_accounts_include_all_hops() {
    let (cache, pool_a, pool_b, token) = setup_two_pool_cache();
    let sol = config::sol_mint();
    let guard_id = Pubkey::new_unique();
    let builder = BundleBuilder::new(Keypair::new(), cache, Some(guard_id));

    let route = ArbRoute {
        base_mint: sol,
        input_amount: 1_000_000,
        estimated_profit: 50_000,
        estimated_profit_lamports: 50_000,
        hops: vec![
            RouteHop {
                pool_address: pool_a,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol,
                output_mint: token,
                estimated_output: 1_000_000,
            },
            RouteHop {
                pool_address: pool_b,
                dex_type: DexType::RaydiumCp,
                input_mint: token,
                output_mint: sol,
                estimated_output: 1_050_000,
            },
        ],
    };

    let ixs = builder.build_arb_instructions(&route, 1_000_000).unwrap();
    let arb_ix = ixs.iter().find(|ix| ix.program_id == guard_id).unwrap();

    // remaining_accounts should include accounts from both Orca and Raydium hops
    // Minimum: signer + program_ids + pool accounts + vaults + ATAs
    assert!(arb_ix.accounts.len() >= 10,
        "execute_arb_v2 should have many remaining_accounts, got {}",
        arb_ix.accounts.len());
}

/// Without arb-guard configured, build_arb_instructions should still work
/// (falls back to separate swap IXs without guard wrapping)
#[test]
fn test_no_guard_still_builds_swap_ixs() {
    let (cache, pool_a, pool_b, token) = setup_two_pool_cache();
    let sol = config::sol_mint();
    let builder = BundleBuilder::new(Keypair::new(), cache, None); // no guard

    let route = ArbRoute {
        base_mint: sol,
        input_amount: 1_000_000,
        estimated_profit: 50_000,
        estimated_profit_lamports: 50_000,
        hops: vec![
            RouteHop {
                pool_address: pool_a,
                dex_type: DexType::OrcaWhirlpool,
                input_mint: sol,
                output_mint: token,
                estimated_output: 1_000_000,
            },
            RouteHop {
                pool_address: pool_b,
                dex_type: DexType::RaydiumCp,
                input_mint: token,
                output_mint: sol,
                estimated_output: 1_050_000,
            },
        ],
    };

    let ixs = builder.build_arb_instructions(&route, 1_000_000).unwrap();
    // Should produce individual swap IXs (no guard program)
    assert!(ixs.len() >= 4, "Should have compute budget + ATA + wrap + swaps + unwrap");
}
```

Note: The exact pool setup may need adjustment based on what data each swap IX builder requires. Read the existing test helpers in `arb_guard_cpi.rs` and adapt. The key assertions are:
1. With guard: single `execute_arb_v2` IX (not start_check + swaps + profit_check)
2. Without guard: falls back to separate swap IXs

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test unit arb_guard_cpi`
Expected: FAIL — the current code routes multi-DEX to start_check/profit_check, not execute_arb_v2

- [ ] **Step 3: Commit failing tests**

```bash
git add tests/unit/arb_guard_cpi.rs
git commit -m "test: add failing tests for execute_arb_v2 multi-DEX builder

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: Implement build_execute_arb_v2_ix

**Files:**
- Modify: `src/executor/bundle.rs`

- [ ] **Step 1: Add the build_execute_arb_v2_ix method**

Add a new method to `BundleBuilder` that builds the `execute_arb_v2` instruction. This method:

1. For each hop, calls the existing `build_swap_instruction_with_min_out` to get a full `Instruction`
2. Decomposes each `Instruction` into `(program_id, accounts, data)` 
3. Collects all accounts into one flat `remaining_accounts` vec
4. Builds `HopV2Params` for each hop with correct indices
5. Serializes `ArbV2Params` using Anchor Borsh format
6. Returns a single instruction to the arb-guard program

The Anchor discriminator for `execute_arb_v2` is `[141, 60, 173, 81, 122, 89, 6, 39]` (from the IDL generated during `anchor build`).

```rust
/// Build a single `execute_arb_v2` CPI instruction that works with ALL DEXes.
/// Decomposes per-hop swap IXs into remaining_accounts + HopV2Params.
pub fn build_execute_arb_v2_ix(
    &self,
    route: &ArbRoute,
    min_amount_out: u64,
) -> Result<Instruction> {
    let guard_program = self.arb_guard_program_id
        .ok_or_else(|| anyhow::anyhow!("arb_guard_program_id not set"))?;

    let signer_pubkey = self.searcher_keypair.pubkey();

    // Build per-hop swap IXs using existing builders
    let mut hop_ixs = Vec::new();
    let last_idx = route.hops.len() - 1;
    for (i, hop) in route.hops.iter().enumerate() {
        let min_out = if i == last_idx { min_amount_out } else { 0 };
        let amount_in = if i == 0 {
            route.input_amount
        } else {
            route.hops[i - 1].estimated_output
        };
        let ix = self.build_swap_instruction_with_min_out(hop, amount_in, min_out)?;
        hop_ixs.push(ix);
    }

    // Flatten all accounts into remaining_accounts
    // First account must be the signer
    let mut remaining_accounts: Vec<AccountMeta> = vec![
        AccountMeta::new(signer_pubkey, true),
    ];
    let mut hop_params = Vec::new();

    for (i, ix) in hop_ixs.iter().enumerate() {
        // Add the DEX program as a remaining account
        let program_id_index = remaining_accounts.len() as u8;
        remaining_accounts.push(AccountMeta::new_readonly(ix.program_id, false));

        // Add hop accounts
        let accounts_start = remaining_accounts.len() as u8;
        for meta in &ix.accounts {
            remaining_accounts.push(meta.clone());
        }
        let accounts_len = ix.accounts.len() as u8;

        // Find the output token account index
        // For circular arbs, the output is the signer's ATA for the hop's output_mint
        let output_mint = route.hops[i].output_mint;
        let output_token_program = if output_mint == addresses::WSOL {
            addresses::SPL_TOKEN
        } else {
            self.state_cache.get_mint_program(&output_mint).unwrap_or(addresses::SPL_TOKEN)
        };
        let output_ata = derive_ata_with_program(&signer_pubkey, &output_mint, &output_token_program);
        let output_token_index = remaining_accounts.iter()
            .position(|a| a.pubkey == output_ata)
            .unwrap_or(0) as u8;

        hop_params.push(HopV2Params {
            program_id_index,
            accounts_start,
            accounts_len,
            output_token_index,
            ix_data: ix.data.clone(),
        });
    }

    // Serialize ArbV2Params using Borsh (Anchor format)
    // Discriminator: [141, 60, 173, 81, 122, 89, 6, 39]
    let params = ArbV2Params {
        min_amount_out,
        hops: hop_params,
    };

    let discriminator: [u8; 8] = [141, 60, 173, 81, 122, 89, 6, 39];
    let mut data = discriminator.to_vec();
    // Borsh serialize the params
    params.serialize(&mut data)
        .map_err(|e| anyhow::anyhow!("Failed to serialize ArbV2Params: {}", e))?;

    Ok(Instruction {
        program_id: guard_program,
        accounts: remaining_accounts,
        data,
    })
}
```

You need to add the `ArbV2Params` and `HopV2Params` structs to `bundle.rs` (or import from a shared location). Since the on-chain program uses Anchor's Borsh, use `borsh::BorshSerialize`:

```rust
use borsh::BorshSerialize;

#[derive(BorshSerialize)]
struct ArbV2Params {
    min_amount_out: u64,
    hops: Vec<HopV2Params>,
}

#[derive(BorshSerialize)]
struct HopV2Params {
    program_id_index: u8,
    accounts_start: u8,
    accounts_len: u8,
    output_token_index: u8,
    ix_data: Vec<u8>,
}
```

Note: `borsh` is already a transitive dependency via `solana-sdk`. If the import doesn't work, add `borsh = "1"` to Cargo.toml.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`

- [ ] **Step 3: Commit**

```bash
git add src/executor/bundle.rs
git commit -m "feat: add build_execute_arb_v2_ix for all-DEX passthrough CPI

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: Wire execute_arb_v2 into build_arb_instructions

**Files:**
- Modify: `src/executor/bundle.rs`

- [ ] **Step 1: Update build_arb_instructions to use execute_arb_v2**

The current `build_arb_instructions` has two paths:
1. **Orca-only CPI path** (lines 56-130): uses old `execute_arb` — **REPLACE with execute_arb_v2 for ALL DEXes**
2. **Start_check/profit_check path** (lines 132-298): separate swap IXs with guard wrapping — **KEEP as fallback when guard is None**

Replace the Orca-only check at line 57:
```rust
// OLD: if self.arb_guard_program_id.is_some() && route.hops.iter().all(|h| h.dex_type == DexType::OrcaWhirlpool)
// NEW: if self.arb_guard_program_id.is_some()
```

This makes the CPI path work for ALL routes when arb-guard is configured.

Then replace the body of that `if` block to use `build_execute_arb_v2_ix` instead of `build_execute_arb_ix`. The structure stays the same:
1. Compute budget
2. ATA creates
3. wSOL wrap
4. `execute_arb_v2` (replaces old `execute_arb`)
5. wSOL unwrap

The second path (start_check/profit_check with size guard) becomes the fallback for when `arb_guard_program_id` is None. Remove the size guard logic and the start_check/profit_check insertion — all guarded routes now go through execute_arb_v2.

- [ ] **Step 2: Run tests**

Run: `cargo test --test unit arb_guard_cpi`
Expected: The new tests from Task 1 should now PASS

- [ ] **Step 3: Run full test suite**

Run: `cargo test --test unit`
Expected: All tests pass. Some old tests may need updating if they expected start_check/profit_check behavior.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: 0 warnings

- [ ] **Step 5: Commit**

```bash
git add src/executor/bundle.rs
git commit -m "feat: use execute_arb_v2 for all DEX routes

All routes with arb-guard configured now use the single passthrough
CPI instruction instead of separate start_check + swaps + profit_check.
Orca-only restriction removed.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: Clean up deprecated code

**Files:**
- Modify: `src/executor/bundle.rs`

- [ ] **Step 1: Remove old build_execute_arb_ix**

Delete the Orca-only `build_execute_arb_ix` method (the one that hardcodes Orca Whirlpool program and 15-account CPI). Keep the new `build_execute_arb_v2_ix`.

Also remove:
- `build_guard_start_check_ix` function
- `build_guard_profit_check_ix` function
- `derive_guard_pda` function (if only used by the removed functions — check first)
- `anchor_discriminator` helper (if only used by old execute_arb — check first)
- `MAX_ACCOUNTS_FOR_GUARD` constant
- `estimate_unique_accounts` function (if only used by size guard — check first)
- The entire start_check/profit_check insertion block in `build_arb_instructions`

**Check before deleting:** grep each function/constant to make sure it's not used elsewhere (tests, other modules). If tests reference them, update the tests.

- [ ] **Step 2: Run full test suite**

Run: `cargo test --test unit && cargo test --features e2e --test e2e`
Expected: All pass. Fix any tests that referenced removed functions.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: 0 warnings (may flag unused imports after cleanup)

- [ ] **Step 4: Commit**

```bash
git add src/executor/bundle.rs tests/
git commit -m "refactor: remove deprecated Orca-only CPI and start_check/profit_check

execute_arb_v2 passthrough handles all DEXes. Old code paths removed.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: Update docs and final validation

**Files:**
- Modify: `CLAUDE.md`
- Modify: `.env.example`

- [ ] **Step 1: Update CLAUDE.md**

In the "Done" section, update:
- Change "arb-guard Phase B: CPI executor for Orca Whirlpool" to "arb-guard Phase B: passthrough CPI executor for all DEXes (execute_arb_v2)"
- Remove "Extend arb-guard CPI executor to all DEX types (currently Orca-only)" from Remaining

- [ ] **Step 2: Remove MIN_ON_CHAIN_PROFIT env var references**

The old start_check/profit_check path used `MIN_ON_CHAIN_PROFIT`. Since execute_arb_v2 uses `min_amount_out` from the route, this env var is no longer needed. Check if it's referenced anywhere and remove.

- [ ] **Step 3: Full test suite**

Run:
```bash
cargo test --test unit
cargo test --features e2e --test e2e
cargo test --test metrics_endpoint
cargo clippy --all-targets -- -D warnings
cargo check --features e2e_surfpool --tests
```

Expected: All pass, 0 warnings

- [ ] **Step 4: Commit and push**

```bash
git add CLAUDE.md .env.example src/ tests/
git commit -m "docs: update for execute_arb_v2 passthrough CPI

Co-Authored-By: Claude <noreply@anthropic.com>"
git push
```
