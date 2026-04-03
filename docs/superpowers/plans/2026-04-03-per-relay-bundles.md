# Per-Relay Bundle Architecture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split monolithic relay.rs into per-relay modules, each owning its own tip accounts + rate limiting + signing + submission. Fix tx-too-large bug by eliminating multi-tip transactions.

**Architecture:** Each relay is a struct implementing a `Relay` trait in its own file under `src/executor/relays/`. `bundle.rs` builds base instructions (no tips). `RelayDispatcher` fires all relays concurrently. Simulator uses single tip amount (not sum of all relay tips).

**Tech Stack:** Rust, async-trait, reqwest, tokio, solana-sdk

---

### Task 1: Create relay trait and types

**Files:**
- Create: `src/executor/relays/mod.rs`

- [ ] **Step 1: Create `src/executor/relays/mod.rs`**

```rust
pub mod jito;
pub mod astralane;
pub mod nozomi;
pub mod bloxroute;
pub mod zeroslot;

use solana_sdk::{
    hash::Hash,
    instruction::Instruction,
    signature::Keypair,
};

/// Result from a relay submission attempt.
#[derive(Debug)]
pub struct RelayResult {
    pub relay_name: String,
    pub bundle_id: Option<String>,
    pub success: bool,
    pub latency_us: u64,
    pub error: Option<String>,
}

/// Every relay implements this trait. Each relay independently:
/// - Checks its own rate limit
/// - Appends its own tip instruction
/// - Signs the transaction
/// - Serializes and sends via HTTP
/// No relay waits for any other relay.
#[async_trait::async_trait]
pub trait Relay: Send + Sync {
    fn name(&self) -> &str;
    fn is_configured(&self) -> bool;
    async fn submit(
        &self,
        base_instructions: &[Instruction],
        tip_lamports: u64,
        signer: &Keypair,
        recent_blockhash: Hash,
    ) -> RelayResult;
}
```

- [ ] **Step 2: Add `async-trait` to Cargo.toml**

Check if `async-trait` is already in dependencies. If not, add it:
```toml
async-trait = "0.1"
```

- [ ] **Step 3: Verify compilation** — `cargo check` (will fail due to missing module files, that's ok)

- [ ] **Step 4: Commit**
```bash
git add src/executor/relays/mod.rs Cargo.toml
git commit -m "feat: relay trait + types for per-relay bundle architecture"
git push origin main
```

---

### Task 2: Implement Jito relay

**Files:**
- Create: `src/executor/relays/jito.rs`

- [ ] **Step 1: Create jito.rs**

Full implementation — owns its 8 tip accounts, rate limiter, auth header, JSON-RPC submission. Builds and signs the transaction itself.

Key details from existing `relay.rs`:
- Endpoint: `{JITO_RELAY_URL}/api/v1/bundles` (JSON-RPC `sendBundle`)
- Auth: optional `x-jito-auth` header from `JITO_AUTH_UUID`
- Rate limit: `JITO_TPS` env var (default 5.0)
- Tip accounts: the 8 hardcoded Jito tip accounts (copy from current bundle.rs lines 43-51)
- Params format: `[encoded_txs, {"encoding": "base64"}]`

The `submit()` method must:
1. Check rate limit (own Instant tracker)
2. Clone base_instructions, append `system_instruction::transfer(signer, tip_account, tip_lamports)`
3. Build `Transaction::new_signed_with_payer`, sign with signer
4. `bincode::serialize` → base64 encode
5. POST JSON-RPC
6. Parse response, return RelayResult

- [ ] **Step 2: Verify compilation** — `cargo check`

- [ ] **Step 3: Commit**
```bash
git add src/executor/relays/jito.rs
git commit -m "feat: Jito relay — own tip accounts, rate limit, sign, submit"
git push origin main
```

---

### Task 3: Implement Astralane relay

**Files:**
- Create: `src/executor/relays/astralane.rs`

- [ ] **Step 1: Create astralane.rs**

Key details from existing relay.rs:
- Endpoint: `ASTRALANE_RELAY_URL` (JSON-RPC `sendBundle`)
- Auth: query param `?api-key=` AND header `api_key` from `ASTRALANE_API_KEY`
- Rate limit: `ASTRALANE_TPS` env var (default 40.0)
- Params: `[encoded_txs, {"encoding": "base64", "revertProtection": true}]`
- Tip accounts: 17 hardcoded Astralane tip accounts (copy from current bundle.rs lines 266-283)
- Keepalive: `getHealth` every 30s (spawn at construction or via separate method)

Same `submit()` pattern as Jito but with Astralane-specific auth and revertProtection.

- [ ] **Step 2: Verify compilation** — `cargo check`

- [ ] **Step 3: Commit**
```bash
git add src/executor/relays/astralane.rs
git commit -m "feat: Astralane relay — own tip accounts, revert protection, keepalive"
git push origin main
```

---

### Task 4: Implement Nozomi, bloXroute, ZeroSlot relays

**Files:**
- Create: `src/executor/relays/nozomi.rs`
- Create: `src/executor/relays/bloxroute.rs`
- Create: `src/executor/relays/zeroslot.rs`

- [ ] **Step 1: Create nozomi.rs**

Jito-compatible JSON-RPC, no auth, uses Jito tip accounts (same 8), `NOZOMI_TPS` default 5.0.

- [ ] **Step 2: Create bloxroute.rs**

Different REST format: `{"transaction": [base64_txs], "useBundle": true}`, auth via `Authorization` header from `BLOXROUTE_AUTH_HEADER`, uses Jito tip accounts, `BLOXROUTE_TPS` default 5.0.

- [ ] **Step 3: Create zeroslot.rs**

Jito-compatible JSON-RPC, no auth, uses Jito tip accounts, `ZEROSLOT_TPS` default 5.0.

- [ ] **Step 4: Verify compilation** — `cargo check`

- [ ] **Step 5: Commit**
```bash
git add src/executor/relays/nozomi.rs src/executor/relays/bloxroute.rs src/executor/relays/zeroslot.rs
git commit -m "feat: Nozomi, bloXroute, ZeroSlot relay modules"
git push origin main
```

---

### Task 5: Create RelayDispatcher

**Files:**
- Create: `src/executor/relay_dispatcher.rs`

- [ ] **Step 1: Create relay_dispatcher.rs**

```rust
use std::sync::Arc;
use solana_sdk::{hash::Hash, instruction::Instruction, signature::Keypair};
use tracing::{info, error};

use super::relays::{Relay, RelayResult};

pub struct RelayDispatcher {
    relays: Vec<Arc<dyn Relay>>,
    signer: Arc<Keypair>,
}

impl RelayDispatcher {
    pub fn new(relays: Vec<Arc<dyn Relay>>, signer: Arc<Keypair>) -> Self {
        Self { relays, signer }
    }

    /// Fire all configured relays concurrently. No relay waits for another.
    /// Returns immediately — results are logged async by each relay task.
    pub fn dispatch(
        &self,
        base_instructions: &[Instruction],
        tip_lamports: u64,
        recent_blockhash: Hash,
        rt: &tokio::runtime::Handle,
    ) {
        for relay in &self.relays {
            if !relay.is_configured() { continue; }
            let relay = relay.clone();
            let ixs = base_instructions.to_vec();
            let signer = self.signer.clone();
            let tip = tip_lamports;
            let bh = recent_blockhash;
            rt.spawn(async move {
                let result = relay.submit(&ixs, tip, &signer, bh).await;
                if result.success {
                    info!("Bundle accepted by {}: id={:?} latency={}us",
                        result.relay_name, result.bundle_id, result.latency_us);
                } else if let Some(ref err) = result.error {
                    tracing::warn!("Bundle REJECTED by {}: {} (latency={}us)",
                        result.relay_name, err, result.latency_us);
                }
            });
        }
    }

    /// Warm up connections to all configured relays.
    pub async fn warmup(&self) {
        for relay in &self.relays {
            if relay.is_configured() {
                info!("Relay configured: {}", relay.name());
            }
        }
    }
}
```

- [ ] **Step 2: Verify compilation** — `cargo check`

- [ ] **Step 3: Commit**
```bash
git add src/executor/relay_dispatcher.rs
git commit -m "feat: RelayDispatcher — concurrent fire-and-forget relay fan-out"
git push origin main
```

---

### Task 6: Refactor bundle.rs — remove tips, return instructions

**Files:**
- Modify: `src/executor/bundle.rs`

- [ ] **Step 1: Replace `build_arb_bundle` with `build_arb_instructions`**

The new function returns `Vec<Instruction>` — no tips, no signing, no serialization:

```rust
/// Build base arb instructions (compute budget + ATA creates + swaps).
/// Does NOT include tips or signing — each relay adds its own tip and signs.
pub fn build_arb_instructions(
    &self,
    route: &ArbRoute,
    min_final_output: u64,
) -> Result<Vec<Instruction>>
```

Remove from bundle.rs:
- `build_arb_bundle` (the old entry point)
- `build_arb_transaction_with_tip` (tips moved to relays)
- `build_tip_instruction` (moved to relays)
- `JITO_TIP_ACCOUNTS` constant (moved to jito.rs)
- `JITO_TIP_FLOOR_REST` constant
- `fetch_tip_floor` method
- `DEFAULT_ASTRALANE_TIP_LAMPORTS`, `astralane_tip_lamports()`, `relay_extra_tips()`
- `tip_account_index` field from BundleBuilder
- All Astralane tip account constants

Keep: compute budget IXs, ATA creation IXs, per-DEX swap IX dispatch, all `build_*_swap_ix` functions.

- [ ] **Step 2: Verify compilation** — `cargo check`

- [ ] **Step 3: Commit**
```bash
git add src/executor/bundle.rs
git commit -m "refactor: bundle.rs returns base instructions — no tips, no signing"
git push origin main
```

---

### Task 7: Update executor/mod.rs and simulator

**Files:**
- Modify: `src/executor/mod.rs`
- Modify: `src/router/simulator.rs`
- Delete: `src/executor/relay.rs`

- [ ] **Step 1: Update `src/executor/mod.rs`**

```rust
pub mod bundle;
pub mod relays;
pub mod relay_dispatcher;

pub use bundle::BundleBuilder;
pub use relay_dispatcher::RelayDispatcher;
pub use relays::RelayResult;
```

- [ ] **Step 2: Remove `relay_extra_tips` from simulator**

In `src/router/simulator.rs`:
- Remove `relay_extra_tips: u64` field
- Remove `with_relay_extra_tips()` method
- Change `total_tip_lamports` to just `jito_tip_lamports` (no extra)
- Update `SimulationResult::Profitable` to use `tip_lamports` instead of `total_tip_lamports`

The simulator now accounts for a single tip only, since each relay builds its own independent tx.

- [ ] **Step 3: Delete `src/executor/relay.rs`**

- [ ] **Step 4: Verify compilation** — `cargo check`

- [ ] **Step 5: Commit**
```bash
git add src/executor/mod.rs src/router/simulator.rs
git rm src/executor/relay.rs
git commit -m "refactor: delete monolithic relay.rs, simplify simulator tip accounting"
git push origin main
```

---

### Task 8: Update main.rs to use new architecture

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Replace MultiRelay with RelayDispatcher**

In main.rs, change relay setup:
```rust
// OLD:
let multi_relay = Arc::new(MultiRelay::new(config.clone()));
multi_relay.warmup().await;
multi_relay.spawn_astralane_keepalive(shutdown_rx.clone());

// NEW:
use solana_mev_bot::executor::relays::{jito::JitoRelay, astralane::AstralaneRelay,
    nozomi::NozomiRelay, bloxroute::BloxrouteRelay, zeroslot::ZeroSlotRelay, Relay};

let mut relays: Vec<Arc<dyn Relay>> = vec![
    Arc::new(JitoRelay::new(&config)),
    Arc::new(AstralaneRelay::new(&config, shutdown_rx.clone())),
    Arc::new(NozomiRelay::new(&config)),
    Arc::new(BloxrouteRelay::new(&config)),
    Arc::new(ZeroSlotRelay::new(&config)),
];

let relay_dispatcher = RelayDispatcher::new(relays, Arc::new(searcher_keypair));
relay_dispatcher.warmup().await;
```

- [ ] **Step 2: Update submission flow**

```rust
// OLD:
match bundle_builder.build_arb_bundle(&route, total_tip_lamports, blockhash) {
    Ok(bundle_txs) => {
        let relay = multi_relay.clone();
        rt.spawn(async move { relay.submit_bundle(&bundle_txs, tip).await; });
    }
}

// NEW:
match bundle_builder.build_arb_instructions(&route, min_final_output) {
    Ok(instructions) => {
        relay_dispatcher.dispatch(&instructions, tip_lamports, blockhash, &rt);
        bundles_submitted += 1;
    }
}
```

Where `min_final_output = route.input_amount + route.estimated_profit_lamports.saturating_sub(tip_lamports)`.

- [ ] **Step 3: Remove `with_relay_extra_tips` from simulator construction**

```rust
// OLD:
let profit_simulator = ProfitSimulator::new(...)
    .with_relay_extra_tips(relay_extra_tips());

// NEW:
let profit_simulator = ProfitSimulator::new(
    state_cache.clone(), config.tip_fraction, config.min_profit_lamports,
);
```

- [ ] **Step 4: Update SimulationResult destructuring**

Change `total_tip_lamports` back to `tip_lamports` in the match arm (or whatever the simulator returns).

- [ ] **Step 5: Verify compilation** — `cargo check`

- [ ] **Step 6: Run all tests** — `cargo test --test unit && cargo test --features e2e --test e2e`

- [ ] **Step 7: Commit**
```bash
git add src/main.rs
git commit -m "feat: main.rs uses RelayDispatcher — per-relay independent bundles"
git push origin main
```

---

### Task 9: Update tests

**Files:**
- Modify: `tests/unit/bundle_profit.rs`
- Modify: `tests/unit/simulator_lst.rs`

- [ ] **Step 1: Update bundle_profit test**

The test calls `build_arb_bundle` — update to call `build_arb_instructions` instead. The test should verify that the returned instructions contain swap IXs but NO tip IXs.

- [ ] **Step 2: Update simulator tests**

Remove references to `relay_extra_tips` and `total_tip_lamports`. The simulator now returns `tip_lamports` (single tip).

- [ ] **Step 3: Run all tests** — `cargo test --test unit`

- [ ] **Step 4: Commit**
```bash
git add tests/
git commit -m "test: update tests for per-relay bundle architecture"
git push origin main
```

---

### Task 10: Live verification

- [ ] **Step 1: Build release** — `cargo build --release`

- [ ] **Step 2: Run with simulation**
```bash
SIMULATE_BUNDLES=true timeout 120 cargo run --release 2>&1 | tee /tmp/per-relay-verify.log
```

- [ ] **Step 3: Check results**
```bash
grep "SIM SUCCESS" /tmp/per-relay-verify.log | wc -l
grep "SIM FAILED" /tmp/per-relay-verify.log | wc -l
grep "accepted\|REJECTED" /tmp/per-relay-verify.log | head -10
grep "could not be decoded" /tmp/per-relay-verify.log | wc -l  # should be 0
```

Expected: SIM SUCCESS count similar to before. Zero "could not be decoded" errors. Bundles accepted by relays (tx fits in 1232 bytes now).
