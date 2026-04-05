# SDK Upgrade + LaserStream Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate from monolithic `solana-sdk 2.2` to modular Solana crates (4.x), replace `yellowstone-grpc-client` with `helius-laserstream`, and upgrade tonic to 0.14.

**Architecture:** Update Cargo.toml with all new dependencies, mechanically replace every `solana_sdk::` import with the correct modular crate import, then rewrite stream.rs to use LaserStream's `subscribe()` API with auto-reconnection and Zstd compression. Delete manual reconnection logic from main.rs.

**Tech Stack:** solana-pubkey 4.1, solana-keypair 3.1, solana-hash 4.0, helius-laserstream 0.1.9, tonic 0.14

---

## File Structure

| File | Responsibility | Changes |
|------|---------------|---------|
| `Cargo.toml` | Dependencies | Remove solana-sdk/yellowstone, add modular crates + helius-laserstream |
| `src/addresses.rs` | Centralized Pubkeys | Pubkey import |
| `src/config.rs` | Configuration | Pubkey import |
| `src/sanctum.rs` | Sanctum bootstrap | Pubkey import |
| `src/rpc_helpers.rs` | RPC utilities | Keypair, Signer, Transaction, Hash, ALT imports |
| `src/main.rs` | Pipeline orchestration | Imports + delete reconnection loop |
| `src/mempool/stream.rs` | Geyser streaming | **Major:** yellowstone → LaserStream migration |
| `src/router/pool.rs` | Pool data structures | Pubkey import |
| `src/router/calculator.rs` | Route discovery | Pubkey import |
| `src/router/simulator.rs` | Profit simulation | Imports |
| `src/executor/bundle.rs` | TX construction | Pubkey, Instruction, system_instruction imports |
| `src/executor/relay_dispatcher.rs` | Relay fan-out | Imports |
| `src/executor/relays/common.rs` | Shared relay helpers | Instruction, Keypair, Hash, Transaction imports |
| `src/executor/relays/*.rs` (5 files) | Individual relays | Pubkey import |
| `src/state/cache.rs` | Pool cache | Pubkey import |
| `src/state/blockhash.rs` | Blockhash cache | Hash import |
| `src/bin/setup_alt.rs` | ALT setup CLI | All imports |
| `tests/unit/*.rs` (16 files) | Unit tests | Pubkey, Keypair, Hash imports |
| `tests/e2e/*.rs` | E2E tests | Pubkey import |
| `tests/e2e_surfpool/*.rs` (5 files) | Surfpool E2E tests | All imports |

---

### Task 1: Update Cargo.toml and fix all src/ imports

**Files:**
- Modify: `Cargo.toml`
- Modify: All 22 files in `src/` (see list above)

- [ ] **Step 1: Update Cargo.toml dependencies**

Replace the Solana and Geyser dependencies:

```toml
# REMOVE these lines:
solana-sdk = "2.2"
yellowstone-grpc-client = "12.1"
yellowstone-grpc-proto = "12.1"
tonic = { version = "0.12", features = ["tls", "tls-webpki-roots"] }

# ADD these lines:
# Solana modular crates
solana-pubkey = "4.1"
solana-keypair = "3.1"
solana-signer = "3.0"
solana-signature = "3.3"
solana-hash = "4.0"
solana-instruction = "3.0"
solana-message = "3.0"
solana-transaction = "3.0"
solana-system-interface = "3.1"
solana-address-lookup-table = "4.0"
solana-program = "4.0"

# LaserStream (replaces yellowstone-grpc-client + yellowstone-grpc-proto)
helius-laserstream = "0.1.9"

# tonic upgrade
tonic = { version = "0.14", features = ["tls", "tls-native-roots"] }
```

Keep all other dependencies unchanged. Also update `[dev-dependencies]` if it references `solana-sdk`.

Note: if `tls-native-roots` feature doesn't exist in tonic 0.14, try `tls-roots` or just `tls`. Check `cargo check` output.

- [ ] **Step 2: Fix src/addresses.rs**

```rust
// Old:
use solana_sdk::pubkey::Pubkey;

// New:
use solana_pubkey::Pubkey;
```

- [ ] **Step 3: Fix src/config.rs**

Two import locations to update (line 2 and line 28):

```rust
// Old:
use solana_sdk::pubkey::Pubkey;

// New:
use solana_pubkey::Pubkey;
```

- [ ] **Step 4: Fix src/state/cache.rs**

```rust
// Old:
use solana_sdk::pubkey::Pubkey;

// New:
use solana_pubkey::Pubkey;
```

- [ ] **Step 5: Fix src/state/blockhash.rs**

```rust
// Old:
use solana_sdk::hash::Hash;

// New:
use solana_hash::Hash;
```

Also check for `Hash::new_unique()` — it may have moved. If it doesn't exist on `solana_hash::Hash`, use `Hash::new_from_array([...])` with random bytes in tests.

- [ ] **Step 6: Fix src/router/pool.rs**

```rust
// Old:
use solana_sdk::pubkey::Pubkey;

// New:
use solana_pubkey::Pubkey;
```

- [ ] **Step 7: Fix src/router/calculator.rs**

```rust
// Old:
use solana_sdk::pubkey::Pubkey;

// New:
use solana_pubkey::Pubkey;
```

- [ ] **Step 8: Fix src/router/simulator.rs**

Read the file first to see exact imports, then replace all `solana_sdk::` with the correct modular crate.

- [ ] **Step 9: Fix src/sanctum.rs**

```rust
// Old:
use solana_sdk::pubkey::Pubkey;

// New:
use solana_pubkey::Pubkey;
```

- [ ] **Step 10: Fix src/executor/bundle.rs**

Current imports (line 2-8):
```rust
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signer::Signer,
    system_instruction,
};
```

Replace with:
```rust
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_system_interface::instruction as system_instruction;
```

Also check for `solana_sdk::system_program::id()` calls — replace with `solana_system_interface::program::id()` or `solana_program::system_program::id()`.

- [ ] **Step 11: Fix src/executor/relay_dispatcher.rs**

Read file, replace imports. Likely needs:
```rust
use solana_hash::Hash;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
```

And `AddressLookupTableAccount` from `solana_address_lookup_table`.

- [ ] **Step 12: Fix src/executor/relays/mod.rs**

```rust
// Old:
use solana_sdk::{
    instruction::Instruction,
    pubkey::Pubkey,
    hash::Hash,
};

// New:
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use solana_hash::Hash;
```

- [ ] **Step 13: Fix src/executor/relays/common.rs**

Read the file. It uses Instruction, AccountMeta, Hash, Keypair, Signer, system_instruction, Message, VersionedMessage, VersionedTransaction, Transaction. Replace all:

```rust
use solana_hash::Hash;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::{v0, Message, VersionedMessage};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_system_interface::instruction as system_instruction;
use solana_transaction::{Transaction, versioned::VersionedTransaction};
```

Note: Check exact paths for `VersionedTransaction` — it may be `solana_transaction::versioned::VersionedTransaction` or directly `solana_transaction::VersionedTransaction`. Let the compiler tell you.

- [ ] **Step 14: Fix 5 relay files (jito, nozomi, bloxroute, zeroslot, astralane)**

Each relay imports `solana_sdk::pubkey::Pubkey` and possibly `Hash`. Replace with:
```rust
use solana_pubkey::Pubkey;
// and if needed:
use solana_hash::Hash;
```

Do all 5 files.

- [ ] **Step 15: Fix src/rpc_helpers.rs**

Read the file. It uses Keypair, Signer, Pubkey, Transaction, Hash, AddressLookupTable, AddressLookupTableAccount. Replace:

```rust
use solana_pubkey::Pubkey;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_hash::Hash;
use solana_transaction::Transaction;
use solana_address_lookup_table::{
    state::AddressLookupTable,
    AddressLookupTableAccount,
};
```

Also fix `Keypair::try_from()` if `from_bytes` is still used anywhere.

- [ ] **Step 16: Fix src/bin/setup_alt.rs**

Read the file. It's a standalone binary. Replace all `solana_sdk::` imports with modular equivalents. Common ones: Pubkey, Keypair, Signer, Transaction, Instruction, system_instruction, Hash.

- [ ] **Step 17: Run cargo check**

```bash
cargo check 2>&1 | head -50
```

Expected: May still fail because `src/mempool/stream.rs` and `src/main.rs` still reference yellowstone. That's Task 2. But all OTHER files should compile.

Fix any remaining errors in the non-stream files until `cargo check` only shows stream.rs and main.rs errors.

- [ ] **Step 18: Commit**

```bash
git add Cargo.toml src/
git commit -m "refactor: migrate to modular Solana crates (4.x) + tonic 0.14

Replace monolithic solana-sdk 2.2 with individual crates:
solana-pubkey, solana-keypair, solana-hash, solana-instruction,
solana-transaction, solana-message, solana-system-interface, etc.
Upgrade tonic from 0.12 to 0.14.

stream.rs and main.rs still reference yellowstone — next commit.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: Migrate stream.rs to LaserStream SDK

**Files:**
- Modify: `src/mempool/stream.rs`

- [ ] **Step 1: Read current stream.rs Geyser connection code**

Read `src/mempool/stream.rs` lines 1-120 to understand the current connection setup, `GeyserStream::new()`, `GeyserStream::start()`, and how the subscription request is built.

- [ ] **Step 2: Update imports**

Remove:
```rust
use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::prelude::{
    subscribe_request_filter_accounts_filter::Filter,
    subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
    SubscribeRequestFilterAccounts, SubscribeRequestFilterAccountsFilter,
};
```

Add:
```rust
use helius_laserstream::{
    subscribe, LaserstreamConfig, ChannelOptions,
    grpc::{
        subscribe_request_filter_accounts_filter::Filter,
        subscribe_update::UpdateOneof, CommitmentLevel, SubscribeRequest,
        SubscribeRequestFilterAccounts, SubscribeRequestFilterAccountsFilter,
    },
};
```

Note: The exact re-export path in `helius_laserstream::grpc::*` may differ. Check the crate docs or source. If the types are at `helius_laserstream::grpc::prelude::*`, use that.

- [ ] **Step 3: Update GeyserStream::start() to use LaserStream**

Current `start()` method does:
1. Build `SubscribeRequest` with account filters
2. Connect via `GeyserGrpcClient::build_from_shared(endpoint).x_token(auth).tls_config(tls).connect()`
3. `client.subscribe_once(request)` → stream
4. Loop: process messages from stream
5. On error → return Err (main.rs handles reconnection)

Replace with:
1. Build `SubscribeRequest` (same as before — the proto types are compatible)
2. Build `LaserstreamConfig::new(endpoint, api_key).with_replay(false).with_channel_options(ChannelOptions::default().with_zstd_compression())`
3. `subscribe(config, request)` → (stream, handle)
4. Loop: process messages from stream (same `UpdateOneof::Account` handling)
5. Auto-reconnection is built into LaserStream — no need to return Err for reconnect

The `start()` method signature may change — it currently returns `Result<()>` with errors triggering reconnection in main.rs. With LaserStream's built-in reconnect, `start()` becomes a long-running infinite loop that only exits on shutdown.

```rust
pub async fn start(
    &self,
    change_tx: crossbeam_channel::Sender<PoolStateChange>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let config = LaserstreamConfig::new(
        self.config.geyser_grpc_url.clone(),
        self.config.geyser_auth_token.clone(),
    )
    .with_replay(false)
    .with_channel_options(
        ChannelOptions::default()
            .with_zstd_compression()
    );

    let request = self.build_subscribe_request();

    let (stream, _handle) = subscribe(config, request);
    futures::pin_mut!(stream);

    while let Some(result) = stream.next().await {
        if *shutdown_rx.borrow() {
            info!("Geyser: shutdown requested");
            break;
        }

        match result {
            Ok(update) => {
                self.process_update(update, &change_tx).await;
            }
            Err(e) => {
                warn!("Geyser stream error (LaserStream will auto-reconnect): {}", 
                      crate::config::redact_url(&e.to_string()));
                // LaserStream handles reconnection internally
                // We just continue the loop
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 4: Extract build_subscribe_request() if not already a method**

The SubscribeRequest construction should be a separate method. It builds the account filters for all 9 DEX programs + Sanctum stake pools. This code doesn't change — just make sure the types are imported from `helius_laserstream::grpc::` instead of `yellowstone_grpc_proto::prelude::`.

- [ ] **Step 5: Update process_update signature**

The `process_update` method receives a `SubscribeUpdate`. The type should be the same from the LaserStream re-export. Update the type path:

```rust
// Old:
update: yellowstone_grpc_proto::prelude::SubscribeUpdate,

// New:
update: helius_laserstream::grpc::SubscribeUpdate,
```

Similarly for `next_message`:
```rust
// Old:
stream: &mut (impl tokio_stream::Stream<Item = Result<yellowstone_grpc_proto::prelude::SubscribeUpdate, yellowstone_grpc_proto::tonic::Status>> + Unpin),

// This method may not be needed anymore since we handle the stream directly in start()
```

- [ ] **Step 6: Remove tls_config and manual connection code**

Delete any `ClientTlsConfig`, `GeyserGrpcClient::build_from_shared()`, `connect()`, `subscribe_once()` code. LaserStream handles all of this internally.

- [ ] **Step 7: Add `futures` crate if not already in Cargo.toml**

Check if `futures` or `futures-util` is needed for `pin_mut!` and `StreamExt`. If not already a dependency:

```toml
futures = "0.3"
```

Or use `tokio_stream` which is already a dependency and provides `StreamExt`.

- [ ] **Step 8: Update remaining Pubkey/type imports in stream.rs**

Replace:
```rust
use solana_sdk::pubkey::Pubkey;
```
with:
```rust
use solana_pubkey::Pubkey;
```

- [ ] **Step 9: Run cargo check on stream.rs**

```bash
cargo check 2>&1 | grep "stream.rs" | head -20
```

Fix any remaining type errors. The most likely issues:
- `SubscribeUpdate` field access may differ slightly
- `UpdateOneof` variant names may differ
- `tonic::Status` type path changes

- [ ] **Step 10: Commit**

```bash
git add src/mempool/stream.rs Cargo.toml
git commit -m "feat: migrate Geyser streaming from yellowstone to LaserStream SDK

Replace yellowstone-grpc-client with helius-laserstream 0.1.9.
Built-in auto-reconnection, Zstd compression (70-80% bandwidth reduction),
and slot-based replay on reconnect. Delete manual connection/TLS setup.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: Delete reconnection logic from main.rs and fix imports

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Read current main.rs Geyser spawn block**

Read `src/main.rs` lines 168-206. This is the reconnection loop with exponential backoff.

- [ ] **Step 2: Simplify to single spawn**

Since LaserStream handles reconnection internally, replace the reconnection loop:

```rust
// Old (lines 172-206):
// Task 1: Geyser streaming with reconnect (async, I/O bound)
let stream_handle = {
    let shutdown_rx = shutdown_rx.clone();
    tokio::spawn(async move {
        let mut backoff = std::time::Duration::from_secs(1);
        const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);
        loop {
            match geyser_stream.start(change_tx.clone(), shutdown_rx.clone()).await {
                Ok(()) => { ... }
                Err(e) => { ... }
            }
            // ... reconnection logic ...
        }
    })
};

// New:
// Task 1: Geyser streaming (LaserStream handles reconnection internally)
let stream_handle = {
    let shutdown_rx = shutdown_rx.clone();
    tokio::spawn(async move {
        if let Err(e) = geyser_stream.start(change_tx.clone(), shutdown_rx.clone()).await {
            error!("Geyser stream fatal error: {}", config::redact_url(&e.to_string()));
        }
        info!("Geyser stream task exited");
    })
};
```

- [ ] **Step 3: Fix main.rs imports**

Replace all `solana_sdk::` imports in main.rs with modular crate equivalents. Read the file to see what's used (likely Pubkey and not much else since we extracted most functions).

- [ ] **Step 4: Run cargo check**

```bash
cargo check 2>&1 | head -20
```

Expected: All src/ files compile. Tests may still fail (Task 4).

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "refactor: simplify Geyser task — LaserStream handles reconnection

Delete manual exponential backoff loop (50 lines). LaserStream SDK
handles reconnection with slot-based replay internally.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: Fix all test imports and verify

**Files:**
- Modify: All test files in `tests/unit/`, `tests/e2e/`, `tests/e2e_surfpool/`

- [ ] **Step 1: Fix unit test imports (16 files)**

Most unit tests only use `solana_sdk::pubkey::Pubkey` and `solana_sdk::signature::Keypair`. Bulk replace:

For each file in `tests/unit/`:
```rust
// Old:
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::hash::Hash;

// New:
use solana_pubkey::Pubkey;
use solana_keypair::Keypair;
use solana_hash::Hash;
```

Files to update (from our grep):
- `arb_guard_cpi.rs`: Pubkey, Keypair
- `blockhash.rs`: Hash
- `bundle_orderbook.rs`: Pubkey
- `bundle_profit.rs`: Pubkey, Keypair
- `bundle_raydium_amm.rs`: Pubkey
- `bundle_real_ix.rs`: Pubkey
- `bundle_sanctum.rs`: Pubkey
- `cache_pair_index.rs`: Pubkey
- `calculator_lst.rs`: Pubkey
- `clmm_tick_pricing.rs`: Pubkey
- `dlmm_bin_pricing.rs`: Pubkey
- `pool_orderbook.rs`: Pubkey
- `pool_sanctum.rs`: Pubkey
- `pricing.rs`: Pubkey
- `simulator_lst.rs`: Pubkey
- `stream_parsing.rs`: Pubkey
- `submission_filter.rs`: Pubkey

- [ ] **Step 2: Fix e2e test imports**

`tests/e2e/lst_pipeline.rs`: Replace `use solana_sdk::pubkey::Pubkey` with `use solana_pubkey::Pubkey`.

- [ ] **Step 3: Fix Surfpool e2e test imports (5 files)**

`tests/e2e_surfpool/arb_guard_cpi.rs`:
```rust
// Old:
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_sdk::system_instruction;
use solana_sdk::hash::Hasher;

// New:
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_system_interface::instruction as system_instruction;
use solana_hash::Hasher;
```

`tests/e2e_surfpool/common.rs`:
```rust
// Replace the solana_sdk::{...} block with individual imports
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_system_interface::instruction as system_instruction;
```

`tests/e2e_surfpool/harness.rs`:
```rust
// Replace solana_sdk block
use solana_hash::Hash;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;
```

Note: `Keypair::from_seed()` uses the `SeedDerivable` trait — check if it's re-exported by `solana_keypair` or needs a separate import (`solana_keypair::SeedDerivable` or `solana_derivation_path::SeedDerivable`).

`tests/e2e_surfpool/dex_swaps.rs`:
```rust
use solana_signer::Signer;
```

`tests/e2e_surfpool/pipeline.rs`:
Read and replace imports as needed.

- [ ] **Step 4: Run all unit tests**

```bash
cargo test --test unit 2>&1 | tail -5
```

Expected: All 146 tests pass.

- [ ] **Step 5: Run clippy**

```bash
cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
```

Expected: Zero warnings.

- [ ] **Step 6: Verify e2e tests compile**

```bash
cargo test --features e2e --test e2e --no-run 2>&1 | tail -3
cargo test --features e2e_surfpool --test e2e_surfpool --no-run 2>&1 | tail -3
```

Expected: Both compile successfully.

- [ ] **Step 7: Commit**

```bash
git add tests/ Cargo.toml
git commit -m "refactor: update all test imports to modular Solana crates

146 unit tests passing, all e2e tests compile.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

- [ ] **Step 8: Update CLAUDE.md**

Update the dependencies section and any references to `solana-sdk 2.2`. Add helius-laserstream mention.

```bash
git add CLAUDE.md
git commit -m "docs: update CLAUDE.md for SDK upgrade + LaserStream

Co-Authored-By: Claude <noreply@anthropic.com>"
```

- [ ] **Step 9: Push all commits**

```bash
git push
```
