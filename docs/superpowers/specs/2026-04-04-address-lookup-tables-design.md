# Address Lookup Tables (ALT)

**Date:** 2026-04-04
**Status:** Approved

## Problem

Arb transactions with 30+ accounts approach the 1232-byte Solana TX limit. Some routes exceed it and get silently dropped. 3-hop routes are impossible. Each account costs 32 bytes in legacy transactions.

## Solution

V0 versioned transactions with a pre-created ALT containing 17 common addresses. Accounts in the ALT cost 1 byte instead of 32. No tip accounts in the ALT (Jito restriction).

## ALT Contents (17 addresses)

| # | Address | Type |
|---|---------|------|
| 1 | `11111111111111111111111111111111` | System Program |
| 2 | `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA` | SPL Token |
| 3 | `TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb` | Token-2022 |
| 4 | `ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL` | ATA Program |
| 5 | `ComputeBudget111111111111111111111111111111` | Compute Budget |
| 6 | `MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr` | Memo Program |
| 7 | `675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8` | Raydium AMM |
| 8 | `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C` | Raydium CP |
| 9 | `CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK` | Raydium CLMM |
| 10 | `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc` | Orca Whirlpool |
| 11 | `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo` | Meteora DLMM |
| 12 | `cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG` | Meteora DAMM v2 |
| 13 | `5ocnV1qiCgaQR8Jb8xWnVbApfaygJ8tNoZfgPwsgx9kx` | Sanctum S Controller |
| 14 | `PhoeNiXZ8ByJGLkxNfZRnkUfjvmuYqLR89jjFHGqdXY` | Phoenix V1 |
| 15 | `MNFSTqtC93rEfYHB6hF82sKdZpUDFWkViLByLd1k1Ms` | Manifest |
| 16 | `So11111111111111111111111111111111111111112` | wSOL Mint |
| 17 | `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` | USDC Mint |

**NOT included:** Tip accounts (Jito, Astralane, etc.) — must stay in static portion.

## Setup: One-Time CLI

New binary `src/bin/setup_alt.rs`:
1. Read `SEARCHER_PRIVATE_KEY` or keypair file from .env
2. If `ALT_ADDRESS` in .env: extend existing ALT with any missing addresses
3. If no `ALT_ADDRESS`: create new ALT, extend with 17 addresses, wait 1 slot
4. Print: `ALT_ADDRESS=<address>` for user to add to .env

Run: `cargo run --bin setup-alt`

## Runtime: Engine Changes

### Startup (main.rs)
- Read `ALT_ADDRESS` from .env (optional — graceful degradation to legacy TX if not set)
- Fetch ALT account via `getAccountInfo`, deserialize with `AddressLookupTable::deserialize`
- Cache as `Arc<AddressLookupTableAccount>` in RelayDispatcher

### Each relay submit() (relays/*.rs)
- If ALT cached: build `v0::Message::try_compile(fee_payer, &instructions, &[alt], blockhash)` → `VersionedTransaction`
- If no ALT: fall back to legacy `Transaction::new_signed_with_payer` (current behavior)
- `try_compile` automatically resolves which accounts can use ALT indices
- Serialize with `bincode::serialize` (same for both types)

### Size awareness
- After serialization, if tx > 1100 bytes: log warning with unresolved accounts
- If tx > 1232 bytes: log error recommending `setup-alt --extend`
- Every 1000 bundles: log ALT compression stats

## Files Changed

- Create: `src/bin/setup_alt.rs`
- Modify: `src/executor/relay_dispatcher.rs` — hold `Option<Arc<AddressLookupTableAccount>>`
- Modify: `src/executor/relays/mod.rs` — add ALT to Relay trait submit()
- Modify: `src/executor/relays/jito.rs` — VersionedTransaction with ALT
- Modify: `src/executor/relays/astralane.rs` — same
- Modify: `src/executor/relays/nozomi.rs` — same
- Modify: `src/executor/relays/bloxroute.rs` — same
- Modify: `src/executor/relays/zeroslot.rs` — same
- Modify: `src/main.rs` — load ALT at startup
- Modify: `src/config.rs` — ALT_ADDRESS env var

## Expected Savings

17 addresses × 31 bytes saved = **527 bytes per transaction**
- Legacy: ~960 bytes for accounts → near 1232 limit
- V0+ALT: ~433 bytes for accounts → comfortable margin
- Enables 3-hop routes with room to spare

## Cost

- ALT creation: ~0.003 SOL rent (17 addresses × 32 + 56 bytes header)
- Rent is recoverable if ALT is closed
- 2 transactions to create + extend (~0.00001 SOL fees)

## Testing

- Surfpool E2E: verify V0 transactions execute correctly on local fork
- Unit test: verify ALT address list matches expected 17 addresses
- Live test: verify relays accept V0 bundles (Jito, Astralane confirmed V0 support)
