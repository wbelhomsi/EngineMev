# Sanctum S Controller SwapExactIn — Shank IX Implementation

**Date:** 2026-04-03
**Status:** Design — pending approval

## Problem

The Sanctum swap IX builder uses an 8-byte Anchor discriminator, but the S Controller is a **Shank program** with a 1-byte discriminant. Every Sanctum swap fails immediately with "invalid instruction data" (218 CU consumed). This blocks 97% of detected arbitrage opportunities (all LST rate arbs).

## Verified On-Chain Data

All addresses below were verified via RPC against mainnet accounts:

| Entity | Address | Verified |
|--------|---------|----------|
| S Controller | `5ocnV1qiCgaQR8Jb8xWnVbApfaygJ8tNoZfgPwsgx9kx` | Program ID |
| Pool State PDA | `AYhux5gJzCoeoc1PoJ1VxwPDe22RwcvpHviLDD1oCGvW` | seeds=["state"] |
| LstStateList PDA | `Gb7m4daakbVbrFLR33FKMDVMHAprRZ66CSYt4bpFwUgS` | seeds=["lst-state-list"] |
| Protocol Fee PDA | `6U8Ve7NuTVq9pb3xEC2ZwxBhceWULUuJn1nSKCTraq5r` | seeds=["protocol-fee"] |
| Pricing Program (ACTIVE) | `s1b6NRXj6ygNu1QMKXh2H9LUR2aPApAAm1UQ2DjdhNV` | From Pool State offset 112 |
| Pricing State PDA | `4T9YzXnmQFMyYi2nrxyXjhtUANavmCkxGCsU3GKaNjwT` | seeds=["state"] on pricing program |
| wSOL Calculator | `wsoGmxQLSvwWpuaidCApxN5kEowLe2HLQLJhCQnj4bE` | Program ID |
| SPL Calculator | `sp1V4h2gWorkGhVcazBc22Hfo2f5sd7jcjT4EDPrWFF` | Program ID |
| SPL Calc State PDA | `7orJ4kDhn1Ewp54j29tBzUWDFGhyimhYi7sxybZcphHd` | seeds=["state"] on SPL calc |
| Marinade Calculator | `mare3SCyfZkAndpBRBeonETmkCCB3TJTTrz8ZN2dnhP` | Program ID |
| Marinade Calc State PDA | `FMbUjYFtqgm4Zfpg7MguZp33RQ3tvkd22NgaCCAs3M6E` | seeds=["state"] on marinade calc |
| SPL Stake Pool Program | `SPoo1Ku8WFXoNDMHPsrGSTSG1Y47rzgn41SLUNakuHy` | Program ID |
| SPL Stake Pool ProgData | `EmiU8AQkB2sswTxVB6aCmsAJftoowZGGDXuytm6X65R3` | BPF Loader PDA |
| Marinade Program | `MarBmsSgKXdrN1egZf5sqe1TMai9K1rChYNDJgjq7aD` | Program ID |
| Marinade ProgData | `4PQH9YmfuKrVyZaibkLYpJZPv2FPaybhq2GAuBcWMSBf` | BPF Loader PDA |
| Marinade State | `8szGkuLTAux9XMgZ2vtY39jVSowEcpBfFfD8hXSEqdGC` | Well-known |
| Jito Stake Pool | `Jito4APyf642JPZPx3hGc6WWJ8zPKtRbRs4P815Awbb` | Well-known |
| BlazeStake Pool (bSOL) | `stk9ApL5HeVAwPLr3TLhDXdZS8ptVu7zp6ov8HFDuMi` | Well-known |

## LST Indices (from on-chain LstStateList)

| Index | LST | Mint | Calculator |
|-------|-----|------|------------|
| 1 | wSOL | `So11111111111111111111111111111111111111112` | wSOL calc |
| 9 | bSOL | `bSo13r4TkiE4KumL71LsHTPpL2euBYLFx6h9HP3piy1` | SPL calc |
| 12 | jitoSOL | `J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn` | SPL calc |
| 17 | mSOL | `mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So` | Marinade calc |

## Design

### 1. Instruction Data (27 bytes)

```
[0]      u8   discriminant = 0x01
[1]      u8   src_lst_value_calc_accs (count of remaining accs for src calculator)
[2]      u8   dst_lst_value_calc_accs (count of remaining accs for dst calculator)
[3..7]   u32  src_lst_index (LE)
[7..11]  u32  dst_lst_index (LE)
[11..19] u64  min_amount_out (LE)
[19..27] u64  amount (LE)
```

### 2. Fixed Accounts (12)

| # | Name | Writable | Signer | Source |
|---|------|----------|--------|--------|
| 0 | signer | no | YES | searcher keypair |
| 1 | src_lst_mint | no | no | route hop input_mint |
| 2 | dst_lst_mint | no | no | route hop output_mint |
| 3 | src_lst_acc | YES | no | ATA(signer, src_mint) |
| 4 | dst_lst_acc | YES | no | ATA(signer, dst_mint) |
| 5 | protocol_fee_accumulator | YES | no | ATA(protocol_fee_pda, dst_mint) |
| 6 | src_lst_token_program | no | no | SPL Token (all LSTs use SPL) |
| 7 | dst_lst_token_program | no | no | SPL Token |
| 8 | pool_state | YES | no | PDA["state"] |
| 9 | lst_state_list | YES | no | PDA["lst-state-list"] |
| 10 | src_pool_reserves | YES | no | ATA(pool_state_pda, src_mint) |
| 11 | dst_pool_reserves | YES | no | ATA(pool_state_pda, dst_mint) |

### 3. Remaining Accounts (variable, 3 groups)

**Group A: Source Calculator** (1 or 5 accounts)
- wSOL: `[wsol_calc_program]` (1 account, calc_accs=1)
- SPL (jitoSOL/bSOL): `[spl_calc_program, spl_calc_state, stake_pool, spl_pool_program, spl_pool_progdata]` (5 accounts, calc_accs=5)
- Marinade (mSOL): `[marinade_calc_program, marinade_calc_state, marinade_state, marinade_program, marinade_progdata]` (5 accounts, calc_accs=5)

**Group B: Destination Calculator** (same pattern, 1 or 5 accounts)

**Group C: Pricing Program** (2 accounts)
- `[pricing_program, pricing_state_pda]` (from on-chain verified pricing program)

### 4. Implementation Plan

**config.rs:** Add new program IDs (pricing program, calculators, stake pools, program data accounts). Fix the pricing program from old to current.

**state/cache.rs:** Add `lst_indices: DashMap<Pubkey, u32>` to store mint→index mapping.

**main.rs:** At startup, fetch LstStateList via getAccountInfo, parse 80-byte entries, populate lst_indices cache. Re-enable SanctumInfinity in can_submit_route().

**bundle.rs:** Rewrite `build_sanctum_swap_ix` and `sanctum_swap_accounts` with correct Shank format. The function needs `lst_indices` lookup from cache + per-LST calculator account resolution.

### 5. Approach: Hardcoded Calculator Accounts

Since we only support 4 LSTs (wSOL, jitoSOL, mSOL, bSOL), hardcode the calculator remaining accounts for each. The LST indices are fetched at startup from on-chain LstStateList (they can change if LSTs are added/removed).

This avoids building a generic calculator account resolver while covering our use case.

## Testing

- Unit test: verify 27-byte data layout with correct fields
- Unit test: verify account count (20-21 depending on LST pair)
- Unit test: verify jitoSOL→wSOL and wSOL→jitoSOL account lists
- Live test: SIMULATE_BUNDLES=true should show SIM SUCCESS for Sanctum routes
