# Sanctum Shank IX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace broken Anchor-based Sanctum swap IX with correct Shank format (1-byte discriminant, 27-byte data, 12 fixed + variable remaining accounts), unlocking 97% of detected arb opportunities.

**Architecture:** Hardcode calculator remaining accounts for 4 known LSTs (wSOL, jitoSOL, mSOL, bSOL). Fetch LST indices from on-chain LstStateList at startup. Fix pricing program to on-chain verified address.

**Tech Stack:** Rust, solana-sdk, reqwest (RPC), DashMap

---

### Task 1: Add Sanctum static addresses to config.rs

**Files:**
- Modify: `src/config.rs:24-71` (programs module) and `src/config.rs:74-119` (statics)

- [ ] **Step 1: Add new program IDs and static addresses**

Add these to the `programs` module in `src/config.rs`, after the existing `MANIFEST` entry:

```rust
// Inside pub mod programs { ... }

// Fix: pricing program updated from old f1tU... to on-chain verified s1b6...
static SANCTUM_PRICING: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("s1b6NRXj6ygNu1QMKXh2H9LUR2aPApAAm1UQ2DjdhNV").unwrap()
});

pub fn sanctum_pricing() -> Pubkey { *SANCTUM_PRICING }
```

Replace the old `SANCTUM_FLAT_FEE_PRICING` static and `sanctum_flat_fee_pricing()` function with the new `SANCTUM_PRICING` / `sanctum_pricing()`. Update any call sites.

Then add these statics below the `programs` module (after the existing LST mint statics):

```rust
// ─── Sanctum static addresses (verified on-chain 2026-04-03) ──────────────

static WSOL_CALCULATOR: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("wsoGmxQLSvwWpuaidCApxN5kEowLe2HLQLJhCQnj4bE").unwrap()
});

// SPL Stake Pool Calculator accounts
static SPL_CALC_STATE: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("7orJ4kDhn1Ewp54j29tBzUWDFGhyimhYi7sxybZcphHd").unwrap()
});
static SPL_STAKE_POOL_PROGRAM: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("SPoo1Ku8WFXoNDMHPsrGSTSG1Y47rzgn41SLUNakuHy").unwrap()
});
static SPL_STAKE_POOL_PROG_DATA: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("EmiU8AQkB2sswTxVB6aCmsAJftoowZGGDXuytm6X65R3").unwrap()
});
static JITO_STAKE_POOL: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("Jito4APyf642JPZPx3hGc6WWJ8zPKtRbRs4P815Awbb").unwrap()
});
static BLAZE_STAKE_POOL: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("stk9ApL5HeVAwPLr3TLhDXdZS8ptVu7zp6ov8HFDuMi").unwrap()
});

// Marinade Calculator accounts
static MARINADE_CALC_STATE: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("FMbUjYFtqgm4Zfpg7MguZp33RQ3tvkd22NgaCCAs3M6E").unwrap()
});
static MARINADE_STATE: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("8szGkuLTAux9XMgZ2vtY39jVSowEcpBfFfD8hXSEqdGC").unwrap()
});
static MARINADE_PROGRAM: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("MarBmsSgKXdrN1egZf5sqe1TMai9K1rChYNDJgjq7aD").unwrap()
});
static MARINADE_PROG_DATA: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("4PQH9YmfuKrVyZaibkLYpJZPv2FPaybhq2GAuBcWMSBf").unwrap()
});

// Pricing program state
static SANCTUM_PRICING_STATE: LazyLock<Pubkey> = LazyLock::new(|| {
    Pubkey::from_str("4T9YzXnmQFMyYi2nrxyXjhtUANavmCkxGCsU3GKaNjwT").unwrap()
});
```

Add public accessor functions for all of them.

- [ ] **Step 2: Add `sanctum_calculator_remaining_accounts` function**

This function returns the remaining accounts for a given LST's SOL value calculator:

```rust
/// Returns (calculator_program, remaining_accounts, calc_accs_count) for a given LST mint.
/// The remaining_accounts are the accounts AFTER the calculator program, with lst_mint skipped.
pub fn sanctum_calculator_accounts(mint: &Pubkey) -> (Pubkey, Vec<Pubkey>, u8) {
    if *mint == sol_mint() {
        // wSOL: just the calculator program, no extra accounts
        (*WSOL_CALCULATOR, vec![], 1)
    } else if *mint == *JITOSOL_MINT {
        // jitoSOL: SPL Stake Pool calculator
        (*SPL_STAKE_POOL_CALC, vec![
            *SPL_CALC_STATE,
            *JITO_STAKE_POOL,
            *SPL_STAKE_POOL_PROGRAM,
            *SPL_STAKE_POOL_PROG_DATA,
        ], 5)
    } else if *mint == *BSOL_MINT {
        // bSOL: SPL Stake Pool calculator (different pool)
        (*SPL_STAKE_POOL_CALC, vec![
            *SPL_CALC_STATE,
            *BLAZE_STAKE_POOL,
            *SPL_STAKE_POOL_PROGRAM,
            *SPL_STAKE_POOL_PROG_DATA,
        ], 5)
    } else if *mint == *MSOL_MINT {
        // mSOL: Marinade calculator
        (*MARINADE_CALC, vec![
            *MARINADE_CALC_STATE,
            *MARINADE_STATE,
            *MARINADE_PROGRAM,
            *MARINADE_PROG_DATA,
        ], 5)
    } else {
        // Unknown LST: fallback to wSOL calculator (will fail on-chain but safe)
        (*WSOL_CALCULATOR, vec![], 1)
    }
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check`
Expected: Clean compile (warnings ok)

- [ ] **Step 4: Commit**

```bash
git add src/config.rs
git commit -m "feat: add Sanctum calculator addresses + pricing program fix"
git push origin main
```

---

### Task 2: Add LST index storage to StateCache

**Files:**
- Modify: `src/state/cache.rs:39-67`

- [ ] **Step 1: Add `lst_indices` field to StateCache**

In `src/state/cache.rs`, add to the `StateCache` struct:

```rust
/// Sanctum LstStateList: mint → index in the on-chain list.
/// Populated at startup by fetching the LstStateList account.
lst_indices: Arc<DashMap<Pubkey, u32>>,
```

Initialize in `new()`:
```rust
lst_indices: Arc::new(DashMap::with_capacity(200)),
```

Add accessor methods:
```rust
/// Get the Sanctum LstStateList index for a given mint.
pub fn get_lst_index(&self, mint: &Pubkey) -> Option<u32> {
    self.lst_indices.get(mint).map(|v| *v)
}

/// Set the Sanctum LstStateList index for a mint.
pub fn set_lst_index(&self, mint: Pubkey, index: u32) {
    self.lst_indices.insert(mint, index);
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check`
Expected: Clean compile

- [ ] **Step 3: Commit**

```bash
git add src/state/cache.rs
git commit -m "feat: add lst_indices to StateCache for Sanctum mint→index lookup"
git push origin main
```

---

### Task 3: Bootstrap LstStateList at startup

**Files:**
- Modify: `src/main.rs` (add bootstrap function call + new async function)

- [ ] **Step 1: Add `bootstrap_lst_indices` function to main.rs**

Add this function near `bootstrap_sanctum_pools`:

```rust
/// Fetch the Sanctum LstStateList from on-chain and populate mint→index mapping.
/// Each entry is 80 bytes: padding(16) + mint(32) + calculator(32).
async fn bootstrap_lst_indices(
    client: &reqwest::Client,
    rpc_url: &str,
    state_cache: &crate::state::StateCache,
) -> Result<()> {
    use base64::{engine::general_purpose, Engine as _};

    let s_controller = config::programs::sanctum_s_controller();
    let (lst_state_list_pda, _) = Pubkey::find_program_address(&[b"lst-state-list"], &s_controller);

    let payload = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "getAccountInfo",
        "params": [lst_state_list_pda.to_string(), {"encoding": "base64"}]
    });

    let resp = client.post(rpc_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send().await?
        .json::<serde_json::Value>().await?;

    let b64 = resp["result"]["value"]["data"][0]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("LstStateList account not found"))?;

    let data = general_purpose::STANDARD.decode(b64)?;
    info!("LstStateList: {} bytes", data.len());

    // Parse as array of 80-byte entries, skip 16-byte header
    let header_size = 16;
    if data.len() < header_size { return Ok(()); }
    let entry_data = &data[header_size..];
    let entry_size = 80;
    let count = entry_data.len() / entry_size;

    let mut found = 0;
    for i in 0..count {
        let offset = i * entry_size;
        if offset + entry_size > entry_data.len() { break; }
        // mint is at bytes 16..48 within each entry
        let mint_bytes: [u8; 32] = entry_data[offset + 16..offset + 48]
            .try_into().unwrap_or([0u8; 32]);
        let mint = Pubkey::new_from_array(mint_bytes);
        if mint == Pubkey::default() { continue; }
        state_cache.set_lst_index(mint, i as u32);
        found += 1;
    }

    info!("Bootstrapped {} LST indices from LstStateList", found);
    Ok(())
}
```

- [ ] **Step 2: Call bootstrap at startup in main()**

Add after the existing `bootstrap_sanctum_pools` call (around line 72-76):

```rust
// Bootstrap Sanctum LST indices from on-chain LstStateList
if let Err(e) = bootstrap_lst_indices(&http_client, &config.rpc_url, &state_cache).await {
    warn!("Failed to bootstrap LST indices: {} — Sanctum routes will be disabled", e);
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check`
Expected: Clean compile

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: bootstrap Sanctum LstStateList indices at startup"
git push origin main
```

---

### Task 4: Rewrite Sanctum swap IX builder

**Files:**
- Modify: `src/executor/bundle.rs:493-514` (build_sanctum_swap_ix)
- Modify: `src/executor/bundle.rs:1035-1079` (sanctum_swap_accounts)
- Modify: `src/executor/bundle.rs:360-370` (dispatch — add state_cache param)

- [ ] **Step 1: Rewrite `build_sanctum_swap_ix`**

Replace lines 493-514 in `src/executor/bundle.rs`:

```rust
/// Shank discriminant for Sanctum S Controller SwapExactIn (NOT Anchor).
const SANCTUM_SWAP_EXACT_IN_DISCM: u8 = 0x01;

/// Build a Sanctum Infinity SwapExactIn instruction (Shank format).
///
/// Data: 27 bytes = discm(1) + src_calc_accs(1) + dst_calc_accs(1)
///       + src_lst_index(4) + dst_lst_index(4) + min_amount_out(8) + amount(8)
/// Accounts: 12 fixed + variable remaining (calculator groups + pricing)
pub fn build_sanctum_swap_ix(
    signer: &Pubkey,
    input_mint: &Pubkey,
    output_mint: &Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
    src_lst_index: u32,
    dst_lst_index: u32,
) -> Option<Instruction> {
    let (src_calc_program, src_calc_suffix, src_calc_accs) =
        crate::config::sanctum_calculator_accounts(input_mint);
    let (dst_calc_program, dst_calc_suffix, dst_calc_accs) =
        crate::config::sanctum_calculator_accounts(output_mint);

    // 12 fixed accounts
    let mut accounts = sanctum_swap_accounts_v2(signer, input_mint, output_mint);

    // Group A: Source calculator remaining accounts
    accounts.push(AccountMeta::new_readonly(src_calc_program, false));
    for acc in &src_calc_suffix {
        accounts.push(AccountMeta::new_readonly(*acc, false));
    }

    // Group B: Destination calculator remaining accounts
    accounts.push(AccountMeta::new_readonly(dst_calc_program, false));
    for acc in &dst_calc_suffix {
        accounts.push(AccountMeta::new_readonly(*acc, false));
    }

    // Group C: Pricing program + state
    accounts.push(AccountMeta::new_readonly(crate::config::programs::sanctum_pricing(), false));
    accounts.push(AccountMeta::new_readonly(crate::config::sanctum_pricing_state(), false));

    // 27-byte Shank instruction data
    let mut data = Vec::with_capacity(27);
    data.push(SANCTUM_SWAP_EXACT_IN_DISCM);
    data.push(src_calc_accs);
    data.push(dst_calc_accs);
    data.extend_from_slice(&src_lst_index.to_le_bytes());
    data.extend_from_slice(&dst_lst_index.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    data.extend_from_slice(&amount_in.to_le_bytes());

    Some(Instruction {
        program_id: crate::config::programs::sanctum_s_controller(),
        accounts,
        data,
    })
}
```

- [ ] **Step 2: Rewrite `sanctum_swap_accounts` as `sanctum_swap_accounts_v2`**

Replace the old `sanctum_swap_accounts` function (lines 1035-1079):

```rust
/// Build the 12 fixed accounts for Sanctum SwapExactIn (Shank format).
fn sanctum_swap_accounts_v2(
    signer: &Pubkey,
    input_mint: &Pubkey,
    output_mint: &Pubkey,
) -> Vec<AccountMeta> {
    let s_controller = crate::config::programs::sanctum_s_controller();
    let token_program = *SPL_TOKEN_PROGRAM;

    // PDAs
    let (pool_state_pda, _) = Pubkey::find_program_address(&[b"state"], &s_controller);
    let (lst_state_list_pda, _) = Pubkey::find_program_address(&[b"lst-state-list"], &s_controller);
    let (protocol_fee_pda, _) = Pubkey::find_program_address(&[b"protocol-fee"], &s_controller);

    // ATAs
    let user_src_ata = derive_ata(signer, input_mint);
    let user_dst_ata = derive_ata(signer, output_mint);
    let protocol_fee_accumulator = derive_ata(&protocol_fee_pda, output_mint); // uses DST mint
    let src_pool_reserves = derive_ata(&pool_state_pda, input_mint);
    let dst_pool_reserves = derive_ata(&pool_state_pda, output_mint);

    vec![
        AccountMeta::new_readonly(*signer, true),              // 0: signer
        AccountMeta::new_readonly(*input_mint, false),         // 1: src_lst_mint
        AccountMeta::new_readonly(*output_mint, false),        // 2: dst_lst_mint
        AccountMeta::new(user_src_ata, false),                 // 3: src_lst_acc (writable)
        AccountMeta::new(user_dst_ata, false),                 // 4: dst_lst_acc (writable)
        AccountMeta::new(protocol_fee_accumulator, false),     // 5: protocol_fee_accumulator (writable)
        AccountMeta::new_readonly(token_program, false),       // 6: src_lst_token_program
        AccountMeta::new_readonly(token_program, false),       // 7: dst_lst_token_program
        AccountMeta::new(pool_state_pda, false),               // 8: pool_state (writable)
        AccountMeta::new(lst_state_list_pda, false),           // 9: lst_state_list (writable)
        AccountMeta::new(src_pool_reserves, false),            // 10: src_pool_reserves (writable)
        AccountMeta::new(dst_pool_reserves, false),            // 11: dst_pool_reserves (writable)
    ]
}
```

Keep the old `sanctum_swap_accounts` as a deprecated wrapper or remove it entirely. Update the test imports.

- [ ] **Step 3: Update the dispatch in `build_swap_instruction_with_min_out`**

The Sanctum arm (around line 360) needs to look up LST indices from the state cache. Change the dispatch:

```rust
DexType::SanctumInfinity => {
    let src_idx = self.state_cache.get_lst_index(&hop.input_mint)
        .ok_or_else(|| anyhow::anyhow!("LST index not found for {}", hop.input_mint))?;
    let dst_idx = self.state_cache.get_lst_index(&hop.output_mint)
        .ok_or_else(|| anyhow::anyhow!("LST index not found for {}", hop.output_mint))?;
    build_sanctum_swap_ix(
        &self.searcher_keypair.pubkey(),
        &hop.input_mint,
        &hop.output_mint,
        amount_in,
        minimum_amount_out,
        src_idx,
        dst_idx,
    ).ok_or_else(|| anyhow::anyhow!("Failed to build Sanctum swap IX"))
}
```

- [ ] **Step 4: Re-enable Sanctum in `can_submit_route()`**

In `src/main.rs`, add back `| router::pool::DexType::SanctumInfinity` to the match.

- [ ] **Step 5: Verify compilation**

Run: `cargo check`
Expected: Clean compile

- [ ] **Step 6: Commit**

```bash
git add src/executor/bundle.rs src/main.rs
git commit -m "feat: rewrite Sanctum IX — Shank 1-byte discriminant, 12+variable accounts"
git push origin main
```

---

### Task 5: Update tests

**Files:**
- Modify: `tests/unit/bundle_sanctum.rs`
- Modify: `tests/unit/submission_filter.rs`

- [ ] **Step 1: Rewrite Sanctum tests**

Replace `tests/unit/bundle_sanctum.rs` with tests for the new function signature:

```rust
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use solana_mev_bot::config;
use solana_mev_bot::executor::bundle::build_sanctum_swap_ix;

#[test]
fn test_sanctum_swap_ix_shank_discriminant() {
    let signer = Pubkey::new_unique();
    let jitosol = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let wsol = config::sol_mint();

    let ix = build_sanctum_swap_ix(
        &signer, &jitosol, &wsol,
        1_000_000_000, 990_000_000,
        12, 1, // jitoSOL=12, wSOL=1
    ).expect("Should build");

    // Shank discriminant = 1 byte, value 0x01
    assert_eq!(ix.data[0], 0x01, "Discriminant must be 0x01 (Shank, not Anchor)");
    assert_eq!(ix.data.len(), 27, "Data must be 27 bytes");
}

#[test]
fn test_sanctum_swap_ix_data_layout() {
    let signer = Pubkey::new_unique();
    let jitosol = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let wsol = config::sol_mint();

    let ix = build_sanctum_swap_ix(
        &signer, &jitosol, &wsol,
        2_000_000_000, 1_900_000_000,
        12, 1,
    ).unwrap();

    // src_lst_value_calc_accs at byte 1 (jitoSOL uses SPL calc = 5 accs)
    assert_eq!(ix.data[1], 5);
    // dst_lst_value_calc_accs at byte 2 (wSOL = 1 acc)
    assert_eq!(ix.data[2], 1);
    // src_lst_index at bytes 3..7
    assert_eq!(u32::from_le_bytes(ix.data[3..7].try_into().unwrap()), 12);
    // dst_lst_index at bytes 7..11
    assert_eq!(u32::from_le_bytes(ix.data[7..11].try_into().unwrap()), 1);
    // min_amount_out at bytes 11..19
    assert_eq!(u64::from_le_bytes(ix.data[11..19].try_into().unwrap()), 1_900_000_000);
    // amount at bytes 19..27
    assert_eq!(u64::from_le_bytes(ix.data[19..27].try_into().unwrap()), 2_000_000_000);
}

#[test]
fn test_sanctum_swap_ix_account_count_jitosol_to_wsol() {
    let signer = Pubkey::new_unique();
    let jitosol = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let wsol = config::sol_mint();

    let ix = build_sanctum_swap_ix(
        &signer, &jitosol, &wsol,
        1_000_000_000, 900_000_000,
        12, 1,
    ).unwrap();

    // 12 fixed + 5 src(SPL) + 1 dst(wSOL) + 2 pricing = 20
    assert_eq!(ix.accounts.len(), 20, "jitoSOL->wSOL needs 20 accounts");
    assert!(ix.accounts[0].is_signer);
}

#[test]
fn test_sanctum_swap_ix_account_count_wsol_to_msol() {
    let signer = Pubkey::new_unique();
    let wsol = config::sol_mint();
    let msol = Pubkey::from_str("mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So").unwrap();

    let ix = build_sanctum_swap_ix(
        &signer, &wsol, &msol,
        1_000_000_000, 900_000_000,
        1, 17, // wSOL=1, mSOL=17
    ).unwrap();

    // 12 fixed + 1 src(wSOL) + 5 dst(Marinade) + 2 pricing = 20
    assert_eq!(ix.accounts.len(), 20, "wSOL->mSOL needs 20 accounts");
}

#[test]
fn test_sanctum_swap_ix_program_id() {
    let signer = Pubkey::new_unique();
    let jitosol = Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap();
    let wsol = config::sol_mint();

    let ix = build_sanctum_swap_ix(&signer, &jitosol, &wsol, 1000, 900, 12, 1).unwrap();
    assert_eq!(ix.program_id, config::programs::sanctum_s_controller());
}
```

- [ ] **Step 2: Update submission_filter test**

In `tests/unit/submission_filter.rs`, re-enable Sanctum in the test's `can_submit_route` mirror and flip the `test_sanctum_route_rejected` test back to `test_sanctum_route_accepted`.

- [ ] **Step 3: Run all tests**

Run: `cargo test --test unit`
Expected: All tests pass (including new Sanctum tests)

Run: `cargo test --features e2e --test e2e`
Expected: All 4 e2e tests pass

- [ ] **Step 4: Commit**

```bash
git add tests/unit/bundle_sanctum.rs tests/unit/submission_filter.rs
git commit -m "test: Sanctum Shank IX tests — discriminant, data layout, account counts"
git push origin main
```

---

### Task 6: Live verification

- [ ] **Step 1: Build release**

Run: `cargo build --release`

- [ ] **Step 2: Run with simulation**

Run: `SIMULATE_BUNDLES=true timeout 120 cargo run --release 2>&1 | tee /tmp/sanctum-verify.log`

- [ ] **Step 3: Check results**

```bash
grep "SIM SUCCESS" /tmp/sanctum-verify.log | wc -l
grep "SIM FAILED" /tmp/sanctum-verify.log | grep -oP 'Program \K[A-Za-z0-9]{30,50}(?= failed)' | sort | uniq -c
```

Expected: At least some `SIM SUCCESS` for Sanctum routes. If still `SIM FAILED` with the S Controller, check the error message for clues.

- [ ] **Step 4: Commit verification results to TODO**

Update `docs/TODO-NEXT-SESSION.md` with findings.
