# Next Session TODO

## Status: On-chain TX sent but fails on Token-2022 ATA mismatch. Need Surfpool local testing.

## Critical Bug: Token-2022 ATA Program Mismatch

Pool parser's `token_program_a/b` flags (offsets 878-879 in DLMM) sometimes say SPL Token when the actual mint is Token-2022. This causes:
- ATA CreateIdempotent uses SPL Token → fails with "IncorrectProgramId"
- The ATA address derived with wrong program doesn't match what the swap IX expects

### Fix approach:
Use `get_mint_program()` (RPC-fetched owner) as the SINGLE source of truth for token programs everywhere:
1. ATA creation in `build_arb_instructions` 
2. All swap IX builders (`build_meteora_dlmm_swap_ix`, `build_raydium_cp_swap_ix`, etc.)
3. Never trust pool.extra.token_program_a/b for ATA derivation

### Testing approach:
Use Surfpool for local testing (installing). Fork mainnet state, send txs locally, iterate without mainnet fees.

## What We Proved On-Chain
- Tx structure is correct (9 instructions: CU + ATA creates + wSOL wrap + swaps + unwrap)
- wSOL wrap/SyncNative works
- DLMM Swap2 hop 1 executes successfully (TransferChecked both ways)
- DLMM Swap2 hop 2 fails on bitmap extension or Token-2022 mismatch
- Jito + Astralane accept our bundles (71 + 109 in 30 min)

## Balance
0.75 SOL → ~0.7499 SOL (lost ~5400 lamports in failed tx fees from SEND_PUBLIC tests)

## Architecture Complete
- 85 unit tests, 5 relay modules, 9 DEX IX builders
- Per-relay bundle architecture (each relay owns tip+sign+send)
- SOL-only route filter + wSOL wrap/unwrap
- Sanctum Shank IX verified
