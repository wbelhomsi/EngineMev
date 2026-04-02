# Real Swap Instructions (Raydium CP + DAMM v2) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace placeholder swap IX builders with real instruction builders for Raydium CP and Meteora DAMM v2, add dedup and route filtering, enabling first live bundle submissions.

**Architecture:** Add `PoolExtra` to store vault/config pubkeys from Geyser parsing. New swap IX builders in `bundle.rs` construct real 13-account (CP) and 12-account (DAMM v2) instructions. Route filter gates submission to only routes with real IX support. Dedup skips repeated pool+slot events.

**Tech Stack:** Rust, solana-sdk 2.2, existing crate dependencies.

**Prerequisite:** Run `cargo check` to verify the project compiles before starting.

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `src/router/pool.rs` | Modify | Add `PoolExtra` struct, `extra` field to `PoolState` |
| `src/mempool/stream.rs` | Modify | Store vaults + config in `PoolExtra` for all parsers |
| `src/executor/bundle.rs` | Modify | Real CP and DAMM v2 IX builders, cache access, `can_submit_route()` |
| `src/main.rs` | Modify | Dedup, route filtering, pass cache to bundle builder |
| `tests/unit/bundle_real_ix.rs` | Create | Unit tests for CP and DAMM v2 IX construction |
| `tests/unit/mod.rs` | Modify | Add `mod bundle_real_ix;` |

---

### Task 1: Add PoolExtra to PoolState

**Files:**
- Modify: `src/router/pool.rs`

- [ ] **Step 1: Add PoolExtra struct and field**

In `src/router/pool.rs`, add after the `DexType` impl block (after line 29) and before `PoolState`:

```rust
/// Extra pool data needed for building swap instructions.
/// Not all pools have all fields — depends on DEX type.
#[derive(Debug, Clone, Default)]
pub struct PoolExtra {
    /// Token A vault pubkey (SPL Token account)
    pub vault_a: Option<Pubkey>,
    /// Token B vault pubkey
    pub vault_b: Option<Pubkey>,
    /// AMM config account (Raydium CP)
    pub config: Option<Pubkey>,
    /// Token program for token A (SPL Token or Token-2022)
    pub token_program_a: Option<Pubkey>,
    /// Token program for token B
    pub token_program_b: Option<Pubkey>,
}
```

Add `extra` field to `PoolState`:

```rust
pub struct PoolState {
    // ... existing fields ...
    pub last_slot: u64,
    /// Extra data for swap IX construction (vaults, config, token programs)
    pub extra: PoolExtra,
}
```

Fix all places that construct `PoolState` to include `extra: PoolExtra::default()`. This includes:
- All parsers in `stream.rs` (6 parser functions)
- `bootstrap_sanctum_pools` in `main.rs`
- All test helpers in `tests/unit/` that construct PoolState

- [ ] **Step 2: Verify compilation**

Run: `cargo check`

- [ ] **Step 3: Commit**

```bash
git add src/router/pool.rs
git commit -m "feat: add PoolExtra for vault/config storage in PoolState"
```

---

### Task 2: Store vaults + config in parsers

**Files:**
- Modify: `src/mempool/stream.rs`

- [ ] **Step 1: Update all 6 parsers to populate PoolExtra**

For each parser, set the `extra` field instead of `PoolExtra::default()`:

**parse_orca_whirlpool** — add vault extraction:
```rust
    let vault_a = Pubkey::try_from(&data[133..165]).ok()?;
    let vault_b = Pubkey::try_from(&data[213..245]).ok()?;
    // ... in PoolState construction:
    extra: PoolExtra {
        vault_a: Some(vault_a),
        vault_b: Some(vault_b),
        ..Default::default()
    },
```

**parse_raydium_clmm** — vaults already parsed (137, 169), store them:
```rust
    let vault_0 = Pubkey::try_from(&data[137..169]).ok()?;
    let vault_1 = Pubkey::try_from(&data[169..201]).ok()?;
    extra: PoolExtra {
        vault_a: Some(vault_0),
        vault_b: Some(vault_1),
        ..Default::default()
    },
```

**parse_meteora_dlmm** — vaults at 152, 184:
```rust
    let vault_x = Pubkey::try_from(&data[152..184]).ok()?;
    let vault_y = Pubkey::try_from(&data[184..216]).ok()?;
    extra: PoolExtra {
        vault_a: Some(vault_x),
        vault_b: Some(vault_y),
        ..Default::default()
    },
```

**parse_meteora_damm_v2** — vaults at 232, 264:
```rust
    let vault_a = Pubkey::try_from(&data[232..264]).ok()?;
    let vault_b = Pubkey::try_from(&data[264..296]).ok()?;
    extra: PoolExtra {
        vault_a: Some(vault_a),
        vault_b: Some(vault_b),
        ..Default::default()
    },
```

**parse_raydium_amm_v4** — vaults already returned as tuple, also store in extra:
```rust
    extra: PoolExtra {
        vault_a: Some(coin_vault),
        vault_b: Some(pc_vault),
        ..Default::default()
    },
```

**parse_raydium_cp** — vaults + amm_config + token programs:
```rust
    let amm_config = Pubkey::try_from(&data[8..40]).ok()?;
    let token_0_program = Pubkey::try_from(&data[232..264]).ok()?;
    let token_1_program = Pubkey::try_from(&data[264..296]).ok()?;
    // ... in PoolState:
    extra: PoolExtra {
        vault_a: Some(vault_0),
        vault_b: Some(vault_1),
        config: Some(amm_config),
        token_program_a: Some(token_0_program),
        token_program_b: Some(token_1_program),
    },
```

- [ ] **Step 2: Fix all test PoolState constructions to include `extra: PoolExtra::default()`**

Search all test files for `PoolState {` and add `extra: PoolExtra::default(),`. Import `PoolExtra` where needed.

- [ ] **Step 3: Verify compilation and tests**

Run: `cargo check && cargo test --test unit`

- [ ] **Step 4: Commit**

```bash
git add src/mempool/stream.rs src/main.rs tests/
git commit -m "feat: store vault pubkeys + config in PoolExtra for all parsers"
```

---

### Task 3: Real Raydium CP and DAMM v2 swap IX builders

**Files:**
- Modify: `src/executor/bundle.rs`
- Create: `tests/unit/bundle_real_ix.rs`
- Modify: `tests/unit/mod.rs`

- [ ] **Step 1: Write tests**

Create `tests/unit/bundle_real_ix.rs`:

```rust
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use solana_mev_bot::executor::bundle::{build_raydium_cp_swap_ix, build_damm_v2_swap_ix};
use solana_mev_bot::router::pool::{DexType, PoolState, PoolExtra};

fn make_cp_pool() -> PoolState {
    let mint_0 = Pubkey::new_unique();
    let mint_1 = Pubkey::new_unique();
    PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::RaydiumCp,
        token_a_mint: mint_0,
        token_b_mint: mint_1,
        token_a_reserve: 1_000_000_000,
        token_b_reserve: 2_000_000_000,
        fee_bps: 25,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            config: Some(Pubkey::new_unique()),
            token_program_a: Some(Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()),
            token_program_b: Some(Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()),
        },
    }
}

fn make_damm_pool() -> PoolState {
    PoolState {
        address: Pubkey::new_unique(),
        dex_type: DexType::MeteoraDammV2,
        token_a_mint: Pubkey::new_unique(),
        token_b_mint: Pubkey::new_unique(),
        token_a_reserve: 5_000_000_000,
        token_b_reserve: 10_000_000_000,
        fee_bps: 15,
        current_tick: None,
        sqrt_price_x64: None,
        liquidity: None,
        last_slot: 100,
        extra: PoolExtra {
            vault_a: Some(Pubkey::new_unique()),
            vault_b: Some(Pubkey::new_unique()),
            ..Default::default()
        },
    }
}

#[test]
fn test_raydium_cp_swap_ix_account_count() {
    let pool = make_cp_pool();
    let signer = Pubkey::new_unique();
    let ix = build_raydium_cp_swap_ix(&signer, &pool, pool.token_a_mint, 1_000_000, 900_000);
    assert!(ix.is_some(), "Should build CP swap IX");
    let ix = ix.unwrap();
    assert_eq!(ix.accounts.len(), 13, "Raydium CP swap needs 13 accounts");
    assert!(ix.accounts[0].is_signer, "First account must be signer");
    assert_eq!(ix.data.len(), 24, "Data: 8 disc + 8 amount + 8 min_out");
}

#[test]
fn test_raydium_cp_swap_ix_discriminator() {
    let pool = make_cp_pool();
    let signer = Pubkey::new_unique();
    let ix = build_raydium_cp_swap_ix(&signer, &pool, pool.token_a_mint, 1_000_000, 900_000).unwrap();
    assert_eq!(&ix.data[0..8], &[0x8f, 0xbe, 0x5a, 0xda, 0xc4, 0x1e, 0x33, 0xde]);
}

#[test]
fn test_raydium_cp_swap_ix_returns_none_without_extra() {
    let mut pool = make_cp_pool();
    pool.extra = PoolExtra::default(); // no vaults
    let signer = Pubkey::new_unique();
    assert!(build_raydium_cp_swap_ix(&signer, &pool, pool.token_a_mint, 1_000_000, 0).is_none());
}

#[test]
fn test_damm_v2_swap_ix_account_count() {
    let pool = make_damm_pool();
    let signer = Pubkey::new_unique();
    let ix = build_damm_v2_swap_ix(&signer, &pool, pool.token_a_mint, 2_000_000, 1_800_000);
    assert!(ix.is_some(), "Should build DAMM v2 swap IX");
    let ix = ix.unwrap();
    assert_eq!(ix.accounts.len(), 12, "DAMM v2 swap needs 12 accounts");
    assert_eq!(ix.data.len(), 25, "Data: 8 disc + 8 amount + 8 min_out + 1 mode");
}

#[test]
fn test_damm_v2_swap_ix_discriminator() {
    let pool = make_damm_pool();
    let signer = Pubkey::new_unique();
    let ix = build_damm_v2_swap_ix(&signer, &pool, pool.token_a_mint, 2_000_000, 1_800_000).unwrap();
    assert_eq!(&ix.data[0..8], &[0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88]);
}

#[test]
fn test_damm_v2_swap_ix_swap_mode_exact_in() {
    let pool = make_damm_pool();
    let signer = Pubkey::new_unique();
    let ix = build_damm_v2_swap_ix(&signer, &pool, pool.token_a_mint, 2_000_000, 1_800_000).unwrap();
    assert_eq!(ix.data[24], 0, "swap_mode should be 0 (ExactIn)");
}
```

Add `mod bundle_real_ix;` to `tests/unit/mod.rs`.

- [ ] **Step 2: Implement the IX builders**

Add to `src/executor/bundle.rs` as public functions (after the existing `sanctum_swap_accounts` function):

```rust
/// Build a Raydium CP swap_base_input instruction.
/// Returns None if pool is missing required extra data (vaults, config, token programs).
pub fn build_raydium_cp_swap_ix(
    signer: &Pubkey,
    pool: &crate::router::pool::PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let extra = &pool.extra;
    let vault_a = extra.vault_a?;
    let vault_b = extra.vault_b?;
    let amm_config = extra.config?;
    let token_prog_a = extra.token_program_a?;
    let token_prog_b = extra.token_program_b?;

    let cp_program = Pubkey::from_str("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C").unwrap();

    // Authority PDA: seeds=[], program=CP
    let (authority, _) = Pubkey::find_program_address(&[], &cp_program);

    // Observation PDA: seeds=["observation", pool_id]
    let (observation, _) = Pubkey::find_program_address(
        &[b"observation", pool.address.as_ref()],
        &cp_program,
    );

    // Direction: is input token_0 (mint_a) or token_1 (mint_b)?
    let a_to_b = input_mint == pool.token_a_mint;
    let (input_vault, output_vault) = if a_to_b { (vault_a, vault_b) } else { (vault_b, vault_a) };
    let (input_token_prog, output_token_prog) = if a_to_b { (token_prog_a, token_prog_b) } else { (token_prog_b, token_prog_a) };
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };

    // User ATAs
    let user_input_ata = derive_ata(signer, &input_mint);
    let user_output_ata = derive_ata(signer, &output_mint);

    // Discriminator: swap_base_input
    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&[0x8f, 0xbe, 0x5a, 0xda, 0xc4, 0x1e, 0x33, 0xde]);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());

    let accounts = vec![
        AccountMeta::new(*signer, true),                    // 0: Payer
        AccountMeta::new_readonly(authority, false),         // 1: Authority PDA
        AccountMeta::new_readonly(amm_config, false),        // 2: AMM Config
        AccountMeta::new(pool.address, false),               // 3: Pool State
        AccountMeta::new(user_input_ata, false),             // 4: User Input Token
        AccountMeta::new(user_output_ata, false),            // 5: User Output Token
        AccountMeta::new(input_vault, false),                // 6: Input Vault
        AccountMeta::new(output_vault, false),               // 7: Output Vault
        AccountMeta::new_readonly(input_token_prog, false),  // 8: Input Token Program
        AccountMeta::new_readonly(output_token_prog, false), // 9: Output Token Program
        AccountMeta::new_readonly(input_mint, false),        // 10: Input Mint
        AccountMeta::new_readonly(output_mint, false),       // 11: Output Mint
        AccountMeta::new(observation, false),                // 12: Observation State
    ];

    Some(Instruction { program_id: cp_program, accounts, data })
}

/// Build a Meteora DAMM v2 swap2 instruction.
/// Returns None if pool is missing required extra data (vaults).
pub fn build_damm_v2_swap_ix(
    signer: &Pubkey,
    pool: &crate::router::pool::PoolState,
    input_mint: Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Option<Instruction> {
    let extra = &pool.extra;
    let vault_a = extra.vault_a?;
    let vault_b = extra.vault_b?;

    let damm_program = Pubkey::from_str("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG").unwrap();

    // Authority PDA: seeds=[], program=DAMM_v2
    let (pool_authority, _) = Pubkey::find_program_address(&[], &damm_program);

    // Event Authority PDA: seeds=["__event_authority"]
    let (event_authority, _) = Pubkey::find_program_address(&[b"__event_authority"], &damm_program);

    // Direction
    let a_to_b = input_mint == pool.token_a_mint;
    let (input_vault, output_vault) = if a_to_b { (vault_a, vault_b) } else { (vault_b, vault_a) };
    let output_mint = if a_to_b { pool.token_b_mint } else { pool.token_a_mint };

    let user_input_ata = derive_ata(signer, &input_mint);
    let user_output_ata = derive_ata(signer, &output_mint);

    let token_program = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();

    // Discriminator: swap2
    let mut data = Vec::with_capacity(25);
    data.extend_from_slice(&[0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88]);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    data.push(0u8); // swap_mode = 0 (ExactIn)

    let accounts = vec![
        AccountMeta::new(pool.address, false),               // 0: Pool State
        AccountMeta::new_readonly(pool_authority, false),     // 1: Pool Authority
        AccountMeta::new(input_vault, false),                 // 2: Input Vault
        AccountMeta::new(output_vault, false),                // 3: Output Vault
        AccountMeta::new(user_input_ata, false),              // 4: User Input Token
        AccountMeta::new(user_output_ata, false),             // 5: User Output Token
        AccountMeta::new_readonly(input_mint, false),         // 6: Input Mint
        AccountMeta::new_readonly(output_mint, false),        // 7: Output Mint
        AccountMeta::new_readonly(token_program, false),      // 8: Token Program
        AccountMeta::new_readonly(event_authority, false),    // 9: Event Authority
        AccountMeta::new_readonly(damm_program, false),       // 10: Program
        AccountMeta::new(*signer, true),                      // 11: Payer
    ];

    Some(Instruction { program_id: damm_program, accounts, data })
}
```

- [ ] **Step 3: Wire into build_swap_instruction_with_min_out**

Update the match in `build_swap_instruction_with_min_out` to use the new builders for CP and DAMM v2. The new builders need pool state, so change the method to accept `&StateCache`:

Actually, simpler: the method already has the `hop` which has `pool_address`. Add a `state_cache: &StateCache` parameter to `BundleBuilder` (store it in the struct) and look up pool state when building CP/DAMM IXs.

Add `state_cache: crate::state::StateCache` to `BundleBuilder` struct and `new()`.

Then in `build_swap_instruction_with_min_out`:

```rust
DexType::RaydiumCp => {
    let pool = self.state_cache.get_any(&hop.pool_address)?;
    build_raydium_cp_swap_ix(
        &self.searcher_keypair.pubkey(),
        &pool,
        hop.input_mint,
        hop.estimated_output,
        minimum_amount_out,
    ).ok_or_else(|| anyhow::anyhow!("Missing pool data for Raydium CP"))
}
DexType::MeteoraDammV2 => {
    let pool = self.state_cache.get_any(&hop.pool_address)?;
    build_damm_v2_swap_ix(
        &self.searcher_keypair.pubkey(),
        &pool,
        hop.input_mint,
        hop.estimated_output,
        minimum_amount_out,
    ).ok_or_else(|| anyhow::anyhow!("Missing pool data for DAMM v2"))
}
```

Update `main.rs` where `BundleBuilder::new()` is called to pass `state_cache.clone()`.

- [ ] **Step 4: Run tests**

Run: `cargo test --test unit -- --nocapture`

Expected: All tests pass including new bundle_real_ix tests.

- [ ] **Step 5: Commit**

```bash
git add src/executor/bundle.rs src/main.rs tests/unit/bundle_real_ix.rs tests/unit/mod.rs
git commit -m "feat: real swap IX builders for Raydium CP and Meteora DAMM v2"
```

---

### Task 4: Dedup + route filtering in main.rs

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add dedup**

In the router loop, before processing the change, add dedup. Add at the top of the `spawn_blocking` closure:

```rust
            let mut recent_pools: std::collections::HashMap<Pubkey, u64> = std::collections::HashMap::new();
```

After receiving the change (after line ~197), before the pool lookup:

```rust
                // Dedup: skip if we already processed this pool in this slot
                if recent_pools.get(&change.pool_address) == Some(&change.slot) {
                    continue;
                }
                recent_pools.insert(change.pool_address, change.slot);

                // Evict old entries periodically
                if recent_pools.len() > 10_000 {
                    let current_slot = change.slot;
                    recent_pools.retain(|_, slot| current_slot - *slot < 10);
                }
```

- [ ] **Step 2: Add route filtering**

Add a helper function in `main.rs` (or `bundle.rs`):

```rust
fn can_submit_route(route: &ArbRoute) -> bool {
    route.hops.iter().all(|hop| matches!(
        hop.dex_type,
        DexType::RaydiumCp | DexType::MeteoraDammV2
    ))
}
```

In the router loop, before the bundle build+submit section, add:

```rust
                        if !can_submit_route(&route) {
                            info!("DRY RUN (unsupported DEX in route) — skipping submission");
                            continue;
                        }
```

This goes right after the `if config.dry_run { ... continue; }` block.

- [ ] **Step 3: Verify compilation**

Run: `cargo check`

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: add dedup + route filtering (only submit CP/DAMM v2 routes)"
```

---

### Task 5: Final verification — compile, test, run

- [ ] **Step 1: Run all tests**

Run: `cargo test --test unit && cargo test --features e2e --test e2e`

Expected: All pass.

- [ ] **Step 2: Build release**

Run: `cargo build --release`

- [ ] **Step 3: Quick dry-run test (30s)**

Run: `timeout 30 ./target/release/solana-mev-bot 2>&1 | grep -E "OPPORTUNITY|DRY RUN|unsupported" | head -10`

Verify: opportunities still detected, "unsupported DEX" messages for non-CP/DAMM routes.

- [ ] **Step 4: Push**

```bash
git push origin main
```
