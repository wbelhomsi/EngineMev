# Solana SDK Modular Crate Migration — Status

**Date:** 2026-04-05
**Status:** COMPLETE — solana-sdk 4.0.1 with modular crates

## What's done

- **solana-sdk 2.2 → 4.0.1**: Full migration complete
- **Modular crates added**: solana-system-interface 3.1, solana-message 4.0, solana-address-lookup-table-interface 3.0
- **five8_core workaround**: Added `five8_core = { version = "0.1.2", features = ["std"] }` to fix upstream bug in solana-keypair 3.1.2 where `five8::DecodeError` doesn't impl `std::error::Error` (five8_core's `std` feature gate not activated by five8)
- All 146 unit tests pass, 0 clippy warnings
- LaserStream SDK (helius-laserstream 0.1.9) compatible with solana-sdk 4.0

## Import changes

| Old (solana-sdk 2.2) | New (4.0 + modular) |
|---|---|
| `solana_sdk::system_instruction::transfer()` | `solana_system_interface::instruction::transfer()` |
| `solana_sdk::system_program::id()` | `solana_system_interface::program::id()` |
| `solana_sdk::address_lookup_table::AddressLookupTableAccount` | `solana_message::AddressLookupTableAccount` |
| `solana_sdk::address_lookup_table::state::AddressLookupTable` | `solana_address_lookup_table_interface::state::AddressLookupTable` |
| `solana_sdk::address_lookup_table::instruction` | `solana_address_lookup_table_interface::instruction` |

## Key learnings

1. `solana-sdk 4.0.1` is a thin facade — it no longer re-exports `system_instruction`, `system_program`, or `address_lookup_table`
2. `solana-system-interface` needs `features = ["bincode"]` to get instruction builders like `transfer()`
3. `AddressLookupTableAccount` moved to `solana-message` crate (not the ALT interface crate)
4. `Pubkey` is now an alias for `Address` (from `solana-address` crate) — existing code using `&Pubkey` works with the new `&Address` params
5. `five8_core` upstream bug: `std` feature not activated by `five8` → `DecodeError` missing `Error` impl. Fixed by adding direct dep with `features = ["std"]`

## Files modified

- `Cargo.toml` — updated deps
- `src/executor/bundle.rs` — system_instruction + system_program imports
- `src/executor/relays/common.rs` — system_instruction + ALT imports  
- `src/executor/relays/mod.rs` — ALT import
- `src/executor/relays/jito.rs` — ALT import
- `src/executor/relays/nozomi.rs` — ALT import
- `src/executor/relays/bloxroute.rs` — ALT import
- `src/executor/relays/astralane.rs` — ALT import
- `src/executor/relays/zeroslot.rs` — ALT import
- `src/executor/relay_dispatcher.rs` — ALT import
- `src/rpc_helpers.rs` — ALT + AddressLookupTable imports
- `src/main.rs` — ALT type annotation
- `src/sanctum.rs` — system_program import
- `src/bin/setup_alt.rs` — ALT instruction imports
