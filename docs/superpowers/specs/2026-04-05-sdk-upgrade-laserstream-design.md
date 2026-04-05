# SDK Upgrade + LaserStream Migration

**Date:** 2026-04-05
**Status:** Approved

## Problem

The bot runs on `solana-sdk 2.2` (released March 2025), now 2 major versions behind (`4.0.1` is current). This blocks:
- `helius-laserstream` SDK (requires `solana-pubkey 3.0+`)
- Performance improvements in newer crate versions
- Growing deprecation warnings that mask real issues

We also use `yellowstone-grpc-client 12.1` with manual reconnection logic (~50 lines) that the LaserStream SDK handles natively with Zstd compression (70-80% bandwidth reduction).

## Solution

Aggressive migration in one pass:
1. Replace monolithic `solana-sdk 2.2` with individual modular crates at latest versions
2. Replace `yellowstone-grpc-client` with `helius-laserstream 0.1.9`
3. Upgrade `tonic` from 0.12 to 0.14

## Cargo.toml Changes

### Remove
```toml
solana-sdk = "2.2"
yellowstone-grpc-client = "12.1"
yellowstone-grpc-proto = "12.1"
tonic = { version = "0.12", features = ["tls", "tls-webpki-roots"] }
```

### Add
```toml
# Solana modular crates (replacing monolithic solana-sdk)
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
solana-program = "4.0"           # for Pubkey::find_program_address, system_program::id()

# LaserStream (replaces yellowstone-grpc-client + yellowstone-grpc-proto)
helius-laserstream = "0.1.9"

# tonic upgrade (required by LaserStream internals)
tonic = { version = "0.14", features = ["tls", "tls-native-roots"] }
```

Note: `tls-webpki-roots` may be renamed to `tls-native-roots` in tonic 0.14 — verify during implementation. If neither exists, use `tls` alone and configure roots manually.

## Import Migration Map

Every `solana_sdk::` import changes. Here is the complete mapping:

| Old import | New import | Crate |
|-----------|-----------|-------|
| `solana_sdk::pubkey::Pubkey` | `solana_pubkey::Pubkey` | `solana-pubkey` |
| `solana_sdk::signature::Keypair` | `solana_keypair::Keypair` | `solana-keypair` |
| `solana_sdk::signer::Signer` | `solana_signer::Signer` | `solana-signer` |
| `solana_sdk::signature::Signature` | `solana_signature::Signature` | `solana-signature` |
| `solana_sdk::hash::Hash` | `solana_hash::Hash` | `solana-hash` |
| `solana_sdk::instruction::{Instruction, AccountMeta}` | `solana_instruction::{Instruction, AccountMeta}` | `solana-instruction` |
| `solana_sdk::message::Message` | `solana_message::Message` | `solana-message` |
| `solana_sdk::message::v0` | `solana_message::v0` | `solana-message` |
| `solana_sdk::message::VersionedMessage` | `solana_message::VersionedMessage` | `solana-message` |
| `solana_sdk::transaction::Transaction` | `solana_transaction::Transaction` | `solana-transaction` |
| `solana_sdk::transaction::VersionedTransaction` | `solana_transaction::versioned::VersionedTransaction` | `solana-transaction` |
| `solana_sdk::system_instruction` | `solana_system_interface::instruction as system_instruction` | `solana-system-interface` |
| `solana_sdk::system_program` | `solana_system_interface::program as system_program` | `solana-system-interface` |
| `solana_sdk::address_lookup_table::AddressLookupTableAccount` | `solana_address_lookup_table::AddressLookupTableAccount` | `solana-address-lookup-table` |
| `solana_sdk::address_lookup_table::state::AddressLookupTable` | `solana_address_lookup_table::state::AddressLookupTable` | `solana-address-lookup-table` |
| `solana_sdk::hash::Hasher` | `solana_hash::Hasher` | `solana-hash` |

## Breaking API Changes

| Change | Location | Fix |
|--------|----------|-----|
| `Keypair::from_bytes(b)` removed | `rpc_helpers.rs`, `setup_alt.rs` | `Keypair::try_from(b.as_slice())?` |
| `Hash` inner bytes private (no `.0`) | Grep for `hash.0` | Use `hash.as_bytes()` |
| `Keypair::from_seed` may move | `harness.rs` (test) | `SeedDerivable` trait import |
| `Pubkey` is now alias for `Address` | Transparent — no code change needed | — |
| `system_instruction::transfer` path | All files using system transfers | Import from `solana_system_interface` |

## LaserStream Migration (stream.rs)

### Current flow (yellowstone-grpc-client)
```
1. GeyserGrpcClient::build_from_shared(endpoint)
     .x_token(auth_token)
     .tls_config(ClientTlsConfig::new().with_native_roots())
     .connect().await
2. client.subscribe_once(request).await → (sink, stream)
3. Manual loop: while let Some(msg) = stream.next().await
4. Manual reconnection: on error → exponential backoff (1s → 30s) → goto 1
```

### New flow (helius-laserstream)
```
1. LaserstreamConfig::new(endpoint, api_key)
     .with_replay(false)
     .with_channel_options(ChannelOptions::default().with_zstd_compression())
2. subscribe(config, request) → (stream, handle)
3. Loop: while let Some(msg) = stream.next().await
4. Auto-reconnection built-in (with slot-based replay)
```

### What gets deleted
- Manual `connect_with_retry()` / exponential backoff loop (~50 lines in main.rs)
- `ClientTlsConfig` setup (LaserStream handles TLS internally)
- Manual keepalive ping (LaserStream has built-in ping/pong)

### What stays the same
- `SubscribeRequest` construction (same proto types, re-exported by LaserStream)
- `UpdateOneof::Account` handling (same message format)
- All per-DEX parsers (unchanged — they receive the same account data bytes)
- crossbeam-channel sender (unchanged — parser output format is the same)

### SubscribeRequest type path
```
// Old:
use yellowstone_grpc_proto::prelude::*;

// New:
use helius_laserstream::grpc::*;
// OR:
use helius_laserstream::grpc::{SubscribeRequest, SubscribeRequestFilterAccounts};
```

### Dynamic subscription updates
LaserStream provides `StreamHandle::write(request)` to update subscriptions without reconnecting. We can use this to add/remove DEX program subscriptions dynamically. Not needed for initial migration but useful later.

## Files Changed

| File | Changes |
|------|---------|
| `Cargo.toml` | Remove solana-sdk/yellowstone, add modular crates + helius-laserstream |
| `src/addresses.rs` | `solana_sdk::pubkey::Pubkey` → `solana_pubkey::Pubkey` |
| `src/config.rs` | Update Pubkey import |
| `src/sanctum.rs` | Update Pubkey, serde imports |
| `src/rpc_helpers.rs` | Update Keypair, Signer, Transaction, Hash imports |
| `src/main.rs` | Update imports, delete reconnection logic, use LaserStream API |
| `src/mempool/stream.rs` | **Major:** Replace yellowstone client with LaserStream, update all imports |
| `src/router/pool.rs` | Update Pubkey import |
| `src/router/calculator.rs` | Update Pubkey import |
| `src/router/simulator.rs` | Update imports |
| `src/executor/bundle.rs` | Update Pubkey, Instruction, system_instruction imports |
| `src/executor/relay_dispatcher.rs` | Update imports |
| `src/executor/relays/common.rs` | Update Instruction, Keypair, Hash, Transaction imports |
| `src/executor/relays/*.rs` | Update imports (5 files) |
| `src/state/cache.rs` | Update Pubkey import |
| `src/state/blockhash.rs` | Update Hash import |
| `src/bin/setup_alt.rs` | Update all imports |
| `tests/unit/*.rs` | Update all imports (10+ files) |
| `tests/e2e/*.rs` | Update all imports |
| `tests/e2e_surfpool/*.rs` | Update all imports |

## Execution Strategy

1. **Create git worktree** for isolation
2. **Update Cargo.toml** — all dependency changes at once
3. **`cargo check`** — let the compiler tell us every broken import
4. **Fix imports file by file** — mechanical find-replace using the map above
5. **Migrate stream.rs** to LaserStream API (the only non-mechanical change)
6. **`cargo test --test unit`** — all 146 tests must pass
7. **`cargo clippy --all-targets -- -D warnings`** — zero warnings
8. **Live smoke test** — `DRY_RUN=true` for 2 min, verify Geyser connection + pool parsing
9. **Merge to main**

## Testing

- All 146 unit tests pass (they don't touch network — pure API compatibility check)
- `cargo clippy` clean
- Live DRY_RUN smoke test validates LaserStream connection, TLS, Zstd, and pool state parsing
- Surfpool E2E tests compile (verify with `--no-run`)

## What This Unlocks

- Zstd compression on Geyser stream (70-80% bandwidth reduction)
- Auto-reconnection with slot-based replay (no missed updates on reconnect)
- Dynamic subscription updates via `StreamHandle::write()`
- Latest Solana crate performance improvements
- No more deprecation warnings
- Foundation for future LaserStream-specific features (preprocessed transactions, etc.)
