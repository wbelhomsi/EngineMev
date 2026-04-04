# Surfpool E2E Tests Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** E2E tests using Surfpool (mainnet fork) that verify each DEX swap IX executes on-chain and the full 2-hop arb pipeline works.

**Architecture:** Self-managed Surfpool subprocess forking mainnet. Per-DEX single swap tests + full pipeline tests. Tests send real transactions to Surfpool local RPC and verify on-chain state.

**Tech Stack:** Rust, solana-sdk, reqwest, serde_json, Surfpool CLI, base64

---

### Task 1: Find and verify mainnet pool addresses

**Files:**
- None (research task — output feeds into Task 3)

- [ ] **Step 1: Use RPC to find pool addresses per DEX**

Run this script to find active SOL-paired pools for each DEX. Source `.env` for RPC_URL.

For each DEX program, get recent signatures and extract pool addresses from successful transactions. Or use known pools from the project's live runs (check `/tmp/enginemev-*.log` for pool addresses that appeared in OPPORTUNITY logs).

Known pools from live runs (verify with `getAccountInfo`):
- Look for pools in the log output that had successful first-hop swaps
- The pool `2PyrTE7WQ3fEagUP59y9bhVWxQ6srK7Abt57tWwqhTiL` appeared in recent tests (DLMM)

For each pool found:
1. Verify it exists: `getAccountInfo` returns non-null
2. Verify data size matches DEX type (653=Orca, 1560=CLMM, 904=DLMM, 1112=DAMM v2, 752=AMM, 637=CP)
3. Parse the pool to verify it trades wSOL (`So11111111111111111111111111111111111111112`)
4. Record: pool_address, dex_type, token_a_mint, token_b_mint, data_size

- [ ] **Step 2: Document pool addresses**

Create a list of verified pool addresses. We need at minimum:
- 1 Orca Whirlpool SOL/X pool
- 1 Raydium CP SOL/X pool
- 1 Raydium CLMM SOL/X pool
- 1 DLMM SOL/X pool (ideally with bitmap extension)
- 1 DAMM v2 SOL/X pool

Phoenix, Manifest, and Sanctum can be added later if the AMM tests pass first.

- [ ] **Step 3: Commit pool address research**

---

### Task 2: Add e2e_surfpool feature and test target

**Files:**
- Modify: `Cargo.toml`
- Create: `tests/e2e_surfpool/mod.rs`

- [ ] **Step 1: Add feature and test target to Cargo.toml**

```toml
[features]
default = []
e2e = []
e2e_surfpool = []

[[test]]
name = "e2e_surfpool"
path = "tests/e2e_surfpool/mod.rs"
required-features = ["e2e_surfpool"]
```

- [ ] **Step 2: Create test module file**

Create `tests/e2e_surfpool/mod.rs`:
```rust
mod harness;
mod common;
mod dex_swaps;
mod pipeline;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check --features e2e_surfpool --test e2e_surfpool`
Expected: Compile errors for missing modules (expected at this stage)

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml tests/e2e_surfpool/mod.rs
git commit -m "feat: add e2e_surfpool test target and feature flag"
git push origin main
```

---

### Task 3: Implement Surfpool harness

**Files:**
- Create: `tests/e2e_surfpool/harness.rs`

- [ ] **Step 1: Create harness**

The harness manages Surfpool lifecycle:
- `SurfpoolHarness::start(rpc_url: &str)` — spawns surfpool subprocess on port 18900
- `wait_for_ready()` — polls getHealth every 500ms, timeout 30s
- `rpc_url()` → String
- `send_tx(&self, instructions: &[Instruction], signer: &Keypair) -> TxResult`
  - Gets latest blockhash from Surfpool
  - Builds Transaction, signs, serializes, base64 encodes
  - Sends via `sendTransaction(skipPreflight=true)`
  - Returns TxResult { signature, success, logs, error }
- `get_sol_balance(&self, pubkey: &Pubkey) -> u64`
- `get_token_balance(&self, owner: &Pubkey, mint: &Pubkey) -> u64`
- `Drop` kills the subprocess

All RPC calls use `reqwest::blocking::Client` (tests are sync).

The harness must:
- Read `RPC_URL` from env (required)
- Airdrop 100 SOL to test signer
- Use `--port 18900 --ws-port 18901` to avoid conflicts
- Pass `--ci --no-deploy`

- [ ] **Step 2: Create a smoke test**

In `tests/e2e_surfpool/mod.rs`, add:
```rust
#[test]
fn test_surfpool_harness_starts() {
    let harness = harness::SurfpoolHarness::start();
    let balance = harness.get_sol_balance(&common::test_keypair().pubkey());
    assert!(balance > 0, "Should have airdropped SOL");
}
```

- [ ] **Step 3: Verify it works**

Run: `RPC_URL=$RPC_URL cargo test --features e2e_surfpool --test e2e_surfpool test_surfpool_harness_starts -- --nocapture`

- [ ] **Step 4: Commit**

```bash
git add tests/e2e_surfpool/
git commit -m "feat: Surfpool test harness — subprocess lifecycle + RPC helpers"
git push origin main
```

---

### Task 4: Implement common helpers and pool registry

**Files:**
- Create: `tests/e2e_surfpool/common.rs`

- [ ] **Step 1: Create common helpers**

```rust
// Deterministic test keypair
pub fn test_keypair() -> Keypair { ... }

// Known mainnet pool addresses per DEX type (from Task 1 research)
pub struct KnownPool {
    pub address: Pubkey,
    pub dex_type: DexType,
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub data_size: usize,
}

pub fn known_pools() -> Vec<KnownPool> { ... }

// Build a single swap transaction: compute budget + ATA creates + wSOL wrap + swap + wSOL unwrap
pub fn build_single_swap_tx(
    harness: &SurfpoolHarness,
    pool: &KnownPool,
    amount_lamports: u64,
    signer: &Keypair,
) -> Vec<Instruction> { ... }
```

The `build_single_swap_tx` function needs to:
1. Fetch the pool account from Surfpool (`getAccountInfo`)
2. Parse it using the project's existing parsers (`parse_orca_whirlpool`, etc.)
3. Build the swap IX using the project's existing builders (`build_orca_whirlpool_swap_ix`, etc.)
4. Wrap with compute budget + ATA creates + wSOL wrap/unwrap

- [ ] **Step 2: Commit**

```bash
git add tests/e2e_surfpool/common.rs
git commit -m "feat: e2e common helpers — pool registry + swap tx builder"
git push origin main
```

---

### Task 5: Implement per-DEX swap tests

**Files:**
- Create: `tests/e2e_surfpool/dex_swaps.rs`

- [ ] **Step 1: Implement Orca Whirlpool swap test**

```rust
#[test]
fn test_orca_whirlpool_swap() {
    let harness = SurfpoolHarness::start();
    let signer = test_keypair();
    let pool = known_pools().iter().find(|p| p.dex_type == DexType::OrcaWhirlpool).unwrap();
    
    let sol_before = harness.get_sol_balance(&signer.pubkey());
    let instructions = build_single_swap_tx(&harness, pool, 1_000_000, &signer); // 0.001 SOL
    let result = harness.send_tx(&instructions, &signer);
    
    println!("Logs: {:?}", result.logs);
    assert!(result.success, "Orca swap failed: {:?}", result.error);
    
    // Verify we received output tokens
    let output_mint = if pool.token_a_mint == sol_mint() { pool.token_b_mint } else { pool.token_a_mint };
    let token_balance = harness.get_token_balance(&signer.pubkey(), &output_mint);
    assert!(token_balance > 0, "Should have received output tokens");
}
```

- [ ] **Step 2: Implement remaining DEX swap tests**

Same pattern for: Raydium CP, Raydium CLMM, DLMM, DAMM v2.
Each test is independent — starts its own Surfpool or shares one via `lazy_static`.

- [ ] **Step 3: Run all DEX tests**

Run: `RPC_URL=$RPC_URL cargo test --features e2e_surfpool --test e2e_surfpool dex_swaps -- --nocapture`

- [ ] **Step 4: Commit**

```bash
git add tests/e2e_surfpool/dex_swaps.rs
git commit -m "feat: per-DEX swap e2e tests on Surfpool"
git push origin main
```

---

### Task 6: Implement pipeline tests

**Files:**
- Create: `tests/e2e_surfpool/pipeline.rs`

- [ ] **Step 1: Implement 2-hop arb roundtrip test**

```rust
#[test]
fn test_2hop_arb_roundtrip() {
    let harness = SurfpoolHarness::start();
    let signer = test_keypair();
    
    // Build SOL → USDC on Orca → USDC → SOL on Raydium CP
    // This uses the full BundleBuilder::build_arb_instructions path
    let sol_before = harness.get_sol_balance(&signer.pubkey());
    
    // ... build 2-hop route and instructions ...
    
    let result = harness.send_tx(&instructions, &signer);
    println!("Logs: {:?}", result.logs);
    
    let sol_after = harness.get_sol_balance(&signer.pubkey());
    println!("SOL before: {}, after: {}, diff: {}", sol_before, sol_after, sol_after as i64 - sol_before as i64);
    
    // We don't assert profit (arb may not be profitable on current state)
    // but we assert the tx succeeded (no IX errors)
    assert!(result.success, "2-hop arb failed: {:?}", result.error);
}
```

- [ ] **Step 2: Implement wSOL wrap/unwrap test**

```rust
#[test]
fn test_wsol_wrap_unwrap() {
    // Verify: transfer SOL → wSOL ATA → SyncNative → swap → CloseAccount → SOL back
    // SOL balance after should be close to before (minus fees and slippage)
}
```

- [ ] **Step 3: Implement Token-2022 ATA test**

```rust
#[test]
fn test_token2022_ata_creation() {
    // Find a pool with a Token-2022 mint
    // Build swap → verify ATA created with Token-2022 program
    // Verify no "IncorrectProgramId" error
}
```

- [ ] **Step 4: Run pipeline tests**

Run: `RPC_URL=$RPC_URL cargo test --features e2e_surfpool --test e2e_surfpool pipeline -- --nocapture`

- [ ] **Step 5: Commit**

```bash
git add tests/e2e_surfpool/pipeline.rs
git commit -m "feat: pipeline e2e tests — 2-hop arb, wSOL wrap, Token-2022"
git push origin main
```

---

### Task 7: Fix issues found by tests

- [ ] **Step 1: Run all e2e_surfpool tests**

```bash
RPC_URL=$RPC_URL cargo test --features e2e_surfpool --test e2e_surfpool -- --nocapture 2>&1 | tee /tmp/e2e-results.log
```

- [ ] **Step 2: For each failing test, examine logs and fix the IX builder**

The tests will surface the exact on-chain errors (like they did for Token-2022, bitmap, wSOL). Fix each issue in the IX builders and re-run.

- [ ] **Step 3: Commit all fixes**

```bash
git add -A
git commit -m "fix: issues found by Surfpool e2e tests"
git push origin main
```
