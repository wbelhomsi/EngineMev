# Arb-Guard Passthrough CPI Executor

**Date:** 2026-04-06
**Status:** Approved

## Goal

Replace the Orca-only `execute_arb` with a **DEX-agnostic passthrough CPI executor** that works with all 9 DEXes. The on-chain program doesn't know which DEX it's calling — the client builds the raw swap IXs, the program just invokes them and verifies profit.

## Reference

Transaction `5xUvKLKMx2s1j8vSphKHYB6peHcvE7EsY4F8u7SBKbQzSUw1GQPMy4c2GBquuGqwhcJVkhpaA3oaxQQGshaws1UV` on mainnet shows the exact pattern: one arb executor instruction with 24 remaining_accounts, CPI into Raydium CLMM then another DEX, 128K CU, 2 ALTs.

## Architecture

### On-chain program change: `execute_arb_v2`

New instruction that replaces the Orca-specific `execute_arb`. The old `execute_arb`, `start_check`, and `profit_check` remain for backward compatibility but are deprecated.

```
execute_arb_v2(params: ArbV2Params)

ArbV2Params {
    min_amount_out: u64,        // Final output must exceed this
    hops: Vec<HopV2Params>,     // One per swap
}

HopV2Params {
    program_id_index: u8,       // Index into remaining_accounts for the DEX program
    accounts_start: u8,         // Start index in remaining_accounts for this hop's accounts
    accounts_len: u8,           // How many accounts this hop needs
    output_token_index: u8,     // Index in remaining_accounts of the output token account
    ix_data: Vec<u8>,           // Raw instruction data (client-built, DEX-specific)
}
```

### What the program does per hop:

```
1. pre_balance = read_token_balance(remaining_accounts[hop.output_token_index])
2. program_id = remaining_accounts[hop.program_id_index].key
3. accounts = remaining_accounts[hop.accounts_start .. hop.accounts_start + hop.accounts_len]
4. invoke(&Instruction { program_id, accounts, data: hop.ix_data }, &accounts)
5. post_balance = read_token_balance(remaining_accounts[hop.output_token_index])
6. actual_received = post_balance - pre_balance
7. require!(actual_received > 0, "Swap produced zero output")
8. Rewrite next hop's ix_data amount_in field with actual_received
   (amount_in is always at bytes 1..9 for our DEX IX formats — after the discriminator)
```

After all hops:
```
9. require!(final_balance >= start_balance + min_amount_out, "Insufficient profit")
```

### Step 8 detail: rewriting amount_in

Every DEX swap IX we build has the format `[discriminator(1+), amount_in(8), min_out(8), ...]`. The amount_in is always the first u64 after the discriminator. The discriminator lengths:

| DEX | Discriminator | amount_in offset |
|-----|--------------|-----------------|
| Raydium AMM v4 | 1 byte (16) | bytes 1..9 |
| Raydium CP | 8 bytes (Anchor) | bytes 8..16 |
| Raydium CLMM | 8 bytes (Anchor) | bytes 8..16 |
| Orca Whirlpool | 8 bytes (Anchor) | bytes 8..16 |
| Meteora DLMM | 8 bytes (Anchor) | bytes 8..16 |
| Meteora DAMM v2 | 8 bytes (Anchor) | bytes 8..16 |
| Sanctum | 1 byte (Shank) | varies (complex layout) |
| Phoenix | 1 byte (Shank) | bytes 1..9 |
| Manifest | 8 bytes | bytes 8..16 |

Rather than hardcoding offsets per DEX, the client can pass `amount_in_offset: u8` as part of `HopV2Params`. The program just writes `actual_received` at that offset.

Updated `HopV2Params`:
```
HopV2Params {
    program_id_index: u8,
    accounts_start: u8,
    accounts_len: u8,
    output_token_index: u8,
    amount_in_offset: u8,       // Byte offset of amount_in in ix_data
    ix_data: Vec<u8>,
}
```

### Security

- **Signer verification**: First remaining_account must be the signer and `is_signer == true`
- **No arbitrary program calls**: The program invokes whatever program_id the client provides. This is safe because:
  - The signer signs the transaction — they're authorizing these specific calls
  - Token transfers only work if the signer owns the token accounts
  - A malicious program_id would fail to produce token balance changes
  - The profit check at the end reverts everything if the balance didn't increase
- **Reentrancy**: Not needed for this design (single instruction, no state between calls)
- **Token account validation**: The output_token_index account must be owned by the signer (checked by reading the SPL Token account owner field)

### Transaction structure (what the client sends)

```
IX[0]: SetComputeUnitLimit (400K CU)
IX[1]: CreateATA for intermediate tokens (idempotent, if needed)
IX[2]: Transfer SOL → wSOL ATA + SyncNative (wrap)
IX[3]: execute_arb_v2(params)
         remaining_accounts: [signer, ...hop1_accounts..., ...hop2_accounts...]
IX[4]: CloseAccount wSOL ATA (unwrap)
IX[5]: Transfer tip to Jito/relay
```

5-6 instructions total. The `execute_arb_v2` is the only arb-related instruction.

### Client-side changes (bundle.rs)

The `build_arb_instructions` method changes:
1. Build per-hop swap IXs using existing builders (unchanged)
2. Collect all accounts into one `remaining_accounts` vec
3. Build `ArbV2Params` with hop metadata (offsets, lengths, ix_data)
4. Emit a single `execute_arb_v2` instruction

The existing swap builders (`build_orca_whirlpool_swap_ix`, `build_raydium_amm_swap_ix`, etc.) are reused — their output `Instruction` is decomposed into `(program_id, accounts, data)` for the CPI params.

## What stays the same

- `start_check` / `profit_check` — kept for backward compat, deprecated
- All swap IX builders in bundle.rs — reused, not changed
- Pool parsers in stream.rs — unchanged
- Route calculator and simulator — unchanged

## What changes

| File | Change |
|------|--------|
| `programs/arb-guard/src/lib.rs` | Add `execute_arb_v2` instruction with passthrough CPI |
| `src/executor/bundle.rs` | Update `build_arb_instructions` to use `execute_arb_v2` |
| `tests/unit/arb_guard_cpi.rs` | TDD: new tests for passthrough CPI |
| `tests/e2e_surfpool/arb_guard_cpi.rs` | Update e2e tests |

## Deployment

- Rebuild arb-guard: `anchor build`
- Redeploy to existing program ID (buffer has 500KB, binary is ~183KB — plenty of room)
- Cost: ~0.003 SOL tx fee (rent already paid)
- No data migration needed (GuardState PDA not used by execute_arb_v2)

## Compute budget

Each CPI call costs ~50-80K CU (depending on DEX). With 2 hops + overhead:
- 2-hop arb: ~150-200K CU
- 3-hop arb: ~200-280K CU
- Budget: 400K CU (existing default)

## TDD plan

1. Write unit test: `execute_arb_v2` with 2 mock CPIs, verify balance check
2. Write unit test: verify revert when output < min_amount_out
3. Write unit test: verify actual_received chains correctly between hops
4. Implement on-chain instruction
5. Write client-side test: build_arb_instructions produces single IX with remaining_accounts
6. Implement client-side builder
7. Surfpool e2e: 2-hop arb via CPI on real pools

## Non-Goals

- Removing start_check/profit_check (backward compat)
- Durable nonces (future optimization)
- RequestHeapFrame (only if CU budget requires it)
