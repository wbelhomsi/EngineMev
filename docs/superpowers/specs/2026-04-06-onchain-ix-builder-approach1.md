# Approach 1: On-Chain Per-Hop IX Builder (Future Reference)

**Date:** 2026-04-06
**Status:** Documented for future reference (not implementing now)

## Overview

Instead of the client building instruction data and the on-chain program patching the amount, the on-chain program **constructs** each hop's instruction data from scratch using the actual `interim_amount_in` from the previous hop's balance diff.

This is what a reference router does and is the most robust approach. Documented here in case we need to migrate from Approach 2 (byte rewriting).

## How It Works

```rust
let mut interim_amount_in = params.amount_in; // client's amount for hop 1

for hop in params.hops {
    // Build instruction data ON-CHAIN with real interim_amount_in
    let ix_data = match hop.dex_type {
        DexType::OrcaWhirlpool => {
            let mut data = Vec::with_capacity(43);
            data.extend_from_slice(&ORCA_SWAP_V2_DISC);         // 8 bytes
            data.extend_from_slice(&interim_amount_in.to_le_bytes()); // REAL amount
            data.extend_from_slice(&0u64.to_le_bytes());         // min_out = 0
            data.extend_from_slice(&sqrt_price_limit.to_le_bytes()); // u128
            data.push(1u8); // amount_specified_is_input = true
            data.push(if hop.a_to_b { 1 } else { 0 });
            data.push(0u8); // remaining_accounts_info = None
            data
        }
        DexType::RaydiumCp => { ... }
        DexType::MeteoraDlmm => { ... }
        DexType::PumpSwap => { ... }
        // etc for each DEX
    };

    // Read pre-swap balance
    let pre_balance = get_token_balance(output_account)?;

    // CPI with freshly built data
    invoke(&Instruction { program_id, accounts, data: ix_data }, &account_infos)?;

    // Read post-swap balance — actual output becomes next hop's input
    let post_balance = get_token_balance(output_account)?;
    interim_amount_in = post_balance - pre_balance;
}
```

## Per-DEX Instruction Data Layouts

From a reference router analysis (lib.rs:881-1308):

| DEX | Discriminator | Layout after disc |
|-----|--------------|-------------------|
| Orca Whirlpool | `[0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62]` | amount(u64) + threshold(u64) + sqrt_price(u128) + is_exact_in(bool) + a_to_b(bool) |
| Raydium CLMM | Similar to Orca | amount(u64) + threshold(u64) + sqrt_price(u128) + is_exact_in(bool) |
| Raydium CP | Anchor disc | amount_in(u64) + min_amount_out(u64) |
| Raydium AMM V4 | `[16]` (1 byte, Swap V2) | amount_in(u64) + min_out(u64) |
| Meteora DLMM | Anchor disc | amount_in(u64) + min_amount_out(u64) |
| Meteora DAMM v2 | Anchor disc | amount_in(u64) + min_amount_out(u64) |
| PumpSwap Sell | `[51, 230, 133, 164, 1, 127, 131, 173]` | base_amount_in(u64) + min_quote_out(u64) |
| PumpSwap Buy | `[102, 6, 61, 18, 1, 218, 235, 234]` | base_amount_out(u64) + max_quote_in(u64) + track_volume(u8) |
| Sanctum | Shank disc (1 byte) | Complex layout with calc accounts |
| Phoenix | Shank disc (1 byte) | amount(u64) + ... |
| Manifest | Anchor disc | amount(u64) + ... |

## Per-DEX Parameters Needed On-Chain

The on-chain program needs these per-hop parameters (passed from client):

```rust
struct HopParams {
    dex_type: u8,        // which DEX
    a_to_b: bool,        // swap direction
    // For CLMM/Whirlpool:
    sqrt_price_limit: u128,  // direction-dependent price limit
}
```

The program derives everything else from the DEX type + direction.

## Advantages Over Approach 2 (Byte Rewriting)

1. **Correctness guaranteed** — no risk of writing at wrong offset
2. **Supports DEX-specific parameters** — e.g., sqrt_price_limit for CLMM varies by direction
3. **No client-side ix_data needed** — smaller instruction data in the transaction
4. **ExactOut support** — can compute exact input needed (with on-chain reserve math)

## Disadvantages

1. **DEX-specific code on-chain** — every new DEX requires a program upgrade
2. **Larger program binary** — more code = more rent
3. **Discriminator changes break it** — if a DEX updates their instruction format, program must be redeployed
4. **More CU consumed** — building instruction data on-chain costs compute

## When to Migrate

Consider migrating from Approach 2 to Approach 1 when:
- We need ExactOut support for the last hop
- A DEX changes its instruction format and breaks our byte offsets
- We want to eliminate the `amount_in_offset` field and simplify the client
- CU budget allows the extra on-chain computation

## Reference Implementation

The a reference router `build_swap_ix` function (lib.rs:881-1308) is the reference. Each `PoolType` match arm constructs the full instruction data from scratch. The main swap loop (lib.rs:2254-2298) feeds `interim_amount_in` through the chain.
