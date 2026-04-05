# Solana SDK Modular Crate Migration — Status & Blockers

**Date:** 2026-04-05
**Status:** Blocked — needs research on specific crate issues

## What's done

- LaserStream SDK migration complete (helius-laserstream 0.1.9 replaces yellowstone-grpc-client)
- Zstd compression enabled, auto-reconnection working
- solana-sdk 2.2 still in use (works with LaserStream)

## What we tried

Attempted full migration to individual modular crates (removing solana-sdk entirely). Hit these blockers:

### Blocker 1: solana-keypair 3.1.2 — five8::DecodeError

`solana-keypair 3.1.2` depends on `five8` for base58 encoding. `five8::DecodeError` doesn't implement `std::error::Error`, which breaks compilation when used with `anyhow` or `thiserror`.

**Needs research:** Is there a newer version that fixes this? A patch? A workaround (pin a different five8 version)?

### Blocker 2: solana-sdk 4.0.1 removed re-exports

`solana-sdk 4.0.1` no longer re-exports:
- `system_instruction` module
- `system_program` module  
- `address_lookup_table` module

These were available in 2.x. The replacement crates are unclear:
- `solana-system-interface` — does it have `instruction::transfer()`?
- `solana-address-lookup-table` — does it have `create_lookup_table()` and `extend_lookup_table()`?

### Blocker 3: setup_alt.rs ALT instructions

`src/bin/setup_alt.rs` uses:
```rust
use solana_sdk::address_lookup_table::instruction as alt_ix;
alt_ix::create_lookup_table(authority, payer, recent_slot);
alt_ix::extend_lookup_table(lookup_table, authority, payer, addresses);
```

These instruction builders need to come from somewhere in the 4.x crate ecosystem.

## Research needed next session

1. **solana-keypair**: latest version, five8 bug status, workaround
2. **solana-system-interface**: exact API for `transfer()` — same signature?
3. **solana-address-lookup-table** vs **solana-address-lookup-table-interface**: which has the instruction builders?
4. **solana-program 4.x**: does it re-export `system_program::id()`, `Pubkey::find_program_address()`?
5. **solana-sdk 4.0.1 facade**: can we use it and still get the re-exports? Or is it genuinely broken?
6. **Alternative approach**: use solana-sdk 3.x as a stepping stone? What's the last 3.x version?

## Import mapping (from original research)

| Old (solana-sdk 2.2) | New (modular) | Crate |
|---|---|---|
| `solana_sdk::pubkey::Pubkey` | `solana_pubkey::Pubkey` | solana-pubkey 4.1 |
| `solana_sdk::signature::Keypair` | `solana_keypair::Keypair` | solana-keypair 3.1 |
| `solana_sdk::signer::Signer` | `solana_signer::Signer` | solana-signer 3.0 |
| `solana_sdk::hash::Hash` | `solana_hash::Hash` | solana-hash 4.0 |
| `solana_sdk::instruction::{Instruction, AccountMeta}` | `solana_instruction::{Instruction, AccountMeta}` | solana-instruction 3.0 |
| `solana_sdk::message::*` | `solana_message::*` | solana-message 3.0 |
| `solana_sdk::transaction::*` | `solana_transaction::*` | solana-transaction 3.0 |
| `solana_sdk::system_instruction` | `solana_system_interface::instruction` | solana-system-interface 3.1 |
| `solana_sdk::system_program` | `solana_system_interface::program` | solana-system-interface 3.1 |
| `solana_sdk::address_lookup_table::*` | ??? | solana-address-lookup-table 4.0 |

## Files that need import changes (all src/ + tests/)

22 files in src/, 16 files in tests/unit/, 5 files in tests/e2e_surfpool/, 1 in tests/e2e/.

See `docs/superpowers/specs/2026-04-05-sdk-upgrade-laserstream-design.md` for the full file list and plan.
