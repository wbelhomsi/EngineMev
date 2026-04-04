# Address Lookup Tables Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** V0 versioned transactions with ALT for 527-byte savings per TX, enabling 3-hop routes and reducing Jito auction CU/byte ratio.

**Architecture:** One-time CLI creates ALT with 17 common addresses (no tip accounts). Engine loads ALT at startup, passes to relays. Each relay builds VersionedTransaction with v0::Message::try_compile. Size awareness logs warnings for oversized txs.

**Tech Stack:** Rust, solana-sdk 2.2 (address_lookup_table module), reqwest, base64

---

### Task 1: Create setup-alt CLI binary

**Files:**
- Create: `src/bin/setup_alt.rs`
- Modify: `Cargo.toml` (add [[bin]] target)

- [ ] **Step 1: Add binary target to Cargo.toml**

```toml
[[bin]]
name = "setup-alt"
path = "src/bin/setup_alt.rs"
```

- [ ] **Step 2: Create setup_alt.rs**

The binary:
1. Loads keypair from `SEARCHER_PRIVATE_KEY` or `SEARCHER_KEYPAIR` env/file (reuse `load_keypair` pattern from main.rs)
2. Reads `RPC_URL` from env
3. Checks if `ALT_ADDRESS` is set in env — if so, fetch it and check which of the 17 addresses are missing, extend if needed
4. If no `ALT_ADDRESS`: create new ALT via `create_lookup_table` instruction, extend with all 17 addresses, wait 2 seconds for activation
5. Print `ALT_ADDRESS=<address>` to stdout

The 17 addresses are the exact list from the spec (system programs, DEX program IDs, wSOL, USDC). Use `Pubkey::from_str` for each.

Use `reqwest::blocking::Client` for RPC calls (sendTransaction, getAccountInfo, getSlot). Build transactions with `Transaction::new_signed_with_payer`, serialize with bincode, base64 encode, send via `sendTransaction`.

- [ ] **Step 3: Verify it compiles**

```bash
cargo check --bin setup-alt
```

- [ ] **Step 4: Commit**

```bash
git add src/bin/setup_alt.rs Cargo.toml
git commit -m "feat: setup-alt CLI — creates ALT with 17 common addresses

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin main
```

---

### Task 2: Add ALT to Relay trait and RelayDispatcher

**Files:**
- Modify: `src/executor/relays/mod.rs` (add ALT param to trait)
- Modify: `src/executor/relay_dispatcher.rs` (hold + pass ALT)
- Modify: `src/main.rs` (load ALT at startup)
- Modify: `src/config.rs` (ALT_ADDRESS env var)

- [ ] **Step 1: Add ALT parameter to Relay trait**

In `src/executor/relays/mod.rs`, add the import and update the trait:

```rust
use solana_sdk::address_lookup_table::AddressLookupTableAccount;
use std::sync::Arc;

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
        alt: Option<&AddressLookupTableAccount>,  // NEW
    ) -> RelayResult;
}
```

- [ ] **Step 2: Update RelayDispatcher to hold and pass ALT**

In `src/executor/relay_dispatcher.rs`:

```rust
pub struct RelayDispatcher {
    relays: Vec<Arc<dyn Relay>>,
    signer: Arc<Keypair>,
    alt: Option<Arc<AddressLookupTableAccount>>,  // NEW
}

impl RelayDispatcher {
    pub fn new(
        relays: Vec<Arc<dyn Relay>>,
        signer: Arc<Keypair>,
        alt: Option<Arc<AddressLookupTableAccount>>,  // NEW
    ) -> Self {
        Self { relays, signer, alt }
    }
```

In `dispatch()`, pass the ALT to each relay:

```rust
let alt_ref = self.alt.as_deref();  // Option<&AddressLookupTableAccount>
// ... inside the spawn:
let result = relay.submit(&ixs, tip, &signer, bh, alt_ref).await;
```

Note: since ALT is `Arc`, cloning is cheap. Pass `self.alt.clone()` into the spawned task, then deref inside.

- [ ] **Step 3: Load ALT at startup in main.rs**

Add after component initialization:

```rust
// Load Address Lookup Table if configured
let alt_account = if let Ok(alt_addr_str) = std::env::var("ALT_ADDRESS") {
    match load_alt(&http_client, &config.rpc_url, &alt_addr_str).await {
        Ok(alt) => {
            info!("Loaded ALT {} with {} addresses", alt_addr_str, alt.addresses.len());
            Some(Arc::new(alt))
        }
        Err(e) => {
            warn!("Failed to load ALT: {} — using legacy transactions", e);
            None
        }
    }
} else {
    info!("No ALT_ADDRESS configured — using legacy transactions");
    None
};
```

Add `load_alt` async function that fetches the ALT via `getAccountInfo`, base64 decodes, and calls `AddressLookupTable::deserialize`.

Pass `alt_account` to `RelayDispatcher::new()`.

- [ ] **Step 4: Verify compilation**

```bash
cargo check
```

All 5 relays will fail to compile (trait method signature changed). That's expected — Task 3 fixes them.

- [ ] **Step 5: Commit**

```bash
git add src/executor/relays/mod.rs src/executor/relay_dispatcher.rs src/main.rs
git commit -m "feat: ALT in Relay trait + dispatcher — loaded at startup from ALT_ADDRESS

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin main
```

---

### Task 3: Update all 5 relays to use VersionedTransaction

**Files:**
- Modify: `src/executor/relays/jito.rs`
- Modify: `src/executor/relays/astralane.rs`
- Modify: `src/executor/relays/nozomi.rs`
- Modify: `src/executor/relays/bloxroute.rs`
- Modify: `src/executor/relays/zeroslot.rs`

- [ ] **Step 1: Update each relay's submit() signature**

Add `alt: Option<&AddressLookupTableAccount>` parameter to all 5 relays' `submit()` methods.

- [ ] **Step 2: Replace Transaction building with VersionedTransaction**

In each relay, replace the transaction building block:

```rust
// OLD:
let tx = Transaction::new_signed_with_payer(
    &instructions, Some(&signer.pubkey()), &[signer], recent_blockhash,
);
let tx_bytes = bincode::serialize(&tx)?;

// NEW:
let tx_bytes = if let Some(alt) = alt {
    // V0 versioned transaction with ALT
    use solana_sdk::message::{v0, VersionedMessage};
    use solana_sdk::transaction::VersionedTransaction;
    
    let v0_msg = v0::Message::try_compile(
        &signer.pubkey(),
        &instructions,
        &[alt.clone()],
        recent_blockhash,
    ).map_err(|e| /* return RelayResult error */)?;
    
    let versioned_tx = VersionedTransaction::try_new(
        VersionedMessage::V0(v0_msg), &[signer],
    ).map_err(|e| /* return RelayResult error */)?;
    
    bincode::serialize(&versioned_tx)
        .map_err(|e| /* return RelayResult error */)?
} else {
    // Legacy fallback
    let tx = Transaction::new_signed_with_payer(
        &instructions, Some(&signer.pubkey()), &[signer], recent_blockhash,
    );
    bincode::serialize(&tx)
        .map_err(|e| /* return RelayResult error */)?
};
```

Apply this pattern to all 5 relays. The rest of each relay (base64 encoding, HTTP POST, response parsing) stays identical.

- [ ] **Step 3: Add size awareness logging**

After serialization in each relay, add:

```rust
if tx_bytes.len() > 1232 {
    return RelayResult { error: Some(format!("Tx too large: {} bytes", tx_bytes.len())), ... };
}
if tx_bytes.len() > 1100 {
    tracing::warn!("{}: tx near size limit ({} bytes)", self.name(), tx_bytes.len());
}
```

- [ ] **Step 4: Verify compilation + tests**

```bash
cargo check && cargo test --test unit
```

- [ ] **Step 5: Commit**

```bash
git add src/executor/relays/
git commit -m "feat: all 5 relays use VersionedTransaction with ALT support

Falls back to legacy Transaction when no ALT configured.
Size awareness: warns at >1100 bytes, rejects at >1232 bytes.

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin main
```

---

### Task 4: Test on Surfpool + live run

- [ ] **Step 1: Create ALT on devnet/localnet**

```bash
RPC_URL=$RPC_URL cargo run --bin setup-alt
# Copy the printed ALT_ADDRESS to .env
```

- [ ] **Step 2: Run Surfpool E2E tests with ALT**

```bash
ALT_ADDRESS=<address> RPC_URL=$RPC_URL cargo test --features e2e_surfpool --test e2e_surfpool -- --test-threads=1
```

Expected: Same pass rate as before (4 passed). V0 transactions should be transparent to Surfpool.

- [ ] **Step 3: Run live for 2 minutes**

```bash
ALT_ADDRESS=<address> MIN_PROFIT_LAMPORTS=1000 SKIP_SIMULATOR=true timeout 120 cargo run --release 2>&1 | grep "accepted\|REJECTED\|too large\|near size"
```

Expected: Bundles accepted. Zero "too large" errors. Possible "near size" warnings for 3-hop routes.

- [ ] **Step 4: Commit docs**

```bash
git add docs/
git commit -m "docs: ALT implementation verified on Surfpool + live

Co-Authored-By: Claude <noreply@anthropic.com>"
git push origin main
```
