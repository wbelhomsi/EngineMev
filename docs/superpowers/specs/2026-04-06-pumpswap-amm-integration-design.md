# PumpSwap AMM Integration

**Date:** 2026-04-06
**Status:** Approved

## Goal

Add PumpSwap AMM (`pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA`) as the 10th DEX. This is where 40% of the successful competitor bot's profitable trades come from — graduated Pump.fun memecoin pools with high price dislocations.

## Background

PumpSwap is a constant-product AMM for Pump.fun graduated tokens. Every pool pairs a memecoin (base) against wSOL (quote). 30 bps total fees (20 LP + 5 protocol + 5 creator). Pool accounts are 243-301 bytes. Reserves are in vault token accounts, not pool state (same pattern as Raydium CP).

No on-chain program changes needed — execute_arb_v2 is DEX-agnostic.

## Architecture

Follows the exact same pattern as existing DEXes:

```
Geyser subscription (by program owner + discriminator filter)
  → parse_pumpswap (pool state: mints, vaults, creator)
  → lazy vault fetch (reserves from token accounts)
  → cache upsert
  → route calculator finds cross-DEX arb
  → swap IX builder (buy/sell)
  → execute_arb_v2 CPI
```

## 1. Geyser Subscription

Add `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA` to the monitored programs in `config.rs::monitored_programs()`.

Route incoming accounts by discriminator `[0xf1, 0x9a, 0x6d, 0x04, 0x11, 0xb1, 0x6d, 0xbc]` at offset 0. Cannot route by data size (243-301 overlaps with other DEXes).

## 2. Pool Parser

New function `parse_pumpswap` in `stream.rs`.

### Pool Account Layout

| Offset | Field | Type | Bytes |
|--------|-------|------|-------|
| 0 | Discriminator | u8[8] | 8 |
| 8 | pool_bump | u8 | 1 |
| 9 | index | u16 | 2 |
| 11 | creator | Pubkey | 32 |
| 43 | base_mint | Pubkey | 32 |
| 75 | quote_mint | Pubkey | 32 |
| 107 | lp_mint | Pubkey | 32 |
| 139 | pool_base_token_account (base vault) | Pubkey | 32 |
| 171 | pool_quote_token_account (quote vault) | Pubkey | 32 |
| 203 | lp_supply | u64 | 8 |
| 211 | coin_creator | Pubkey | 32 |
| 243 | is_mayhem_mode | u8 (optional) | 1 |
| 244 | is_cashback_coin | u8 (optional) | 1 |

**Discriminator:** `[0xf1, 0x9a, 0x6d, 0x04, 0x11, 0xb1, 0x6d, 0xbc]`

**Valid data sizes:** Minimum 243 bytes. Known sizes: 243 (no optional fields), 244 (+mayhem), 245 (+mayhem+cashback). Accept any size >= 243 with correct discriminator (some pools may have extra padding from `extend_account`).

**Output:** `(PoolState, (base_vault, quote_vault))` — same as `parse_raydium_cp`.

```rust
PoolState {
    address: pool_address,
    dex_type: DexType::PumpSwap,
    token_a_mint: base_mint,     // memecoin
    token_b_mint: quote_mint,    // always wSOL
    token_a_reserve: 0,          // populated after vault fetch
    token_b_reserve: 0,
    fee_bps: 30,                 // 20 LP + 5 protocol + 5 creator
    extra: PoolExtra {
        vault_a: Some(base_vault),
        vault_b: Some(quote_vault),
        coin_creator: Some(coin_creator),
        is_mayhem_mode: Some(is_mayhem_mode),
        is_cashback_coin: Some(is_cashback_coin),
        token_program_a: None,   // resolved via mint fetch
        token_program_b: Some(SPL_TOKEN), // wSOL is always SPL Token
        ..Default::default()
    },
}
```

### PoolExtra New Fields

Add to `PoolExtra` in `router/pool.rs`:
```rust
pub coin_creator: Option<Pubkey>,
pub is_mayhem_mode: Option<bool>,
pub is_cashback_coin: Option<bool>,
```

### DexType New Variant

Add `PumpSwap` to `DexType` enum in `router/pool.rs`.

## 3. Lazy Vault Fetch

Same pattern as Raydium CP: after parsing pool state, spawn async `getMultipleAccounts` with `dataSlice: { offset: 64, length: 8 }` to read vault balances.

The vault fetch cooldown (2s) and semaphore (10 concurrent) already exist — PumpSwap reuses them.

**Important:** Do NOT emit `PoolStateChange` until vault fetch completes. This prevents the false positive issue where the router sees a pool with zero reserves.

## 4. CPMM Pricing

### Fee Structure (TIERED, not flat)

PumpSwap fees are **dynamic, based on token market cap**. The Fee Program determines the tier at execution time:

| Market Cap (SOL) | LP Fee | Protocol Fee | Creator Fee | Total |
|-------------------|--------|-------------|-------------|-------|
| 0 – 420 | 2 bps | 93 bps | 30 bps | **125 bps** |
| 420 – 4,420 | 15 bps | 35 bps | 50 bps | **100 bps** |
| 4,420 – 9,820 | 15 bps | 25 bps | 10 bps | **50 bps** |
| 9,820 – 98,240 | 20 bps | 10 bps | 10 bps | **40 bps** |
| 98,240+ | 20 bps | 5 bps | 5 bps | **30 bps** |
| Non-canonical pools | 25 bps | 5 bps | 0 bps | **30 bps** |

**For off-chain quoting:** Use worst-case **125 bps** as the fee estimate. This is conservative — we'll underestimate profit but never overestimate. The on-chain program applies the correct fee via the Fee Program CPI.

### Formula

Only the LP fee portion is deducted from input before xy=k. Protocol + creator fees are deducted from output. For conservative quoting, we apply the full worst-case fee to input:

```rust
// Conservative estimate: use 125 bps (worst case) for quoting
// The on-chain program applies the exact correct fee via Fee Program CPI
let fee_bps: u128 = 125; // worst-case for low-cap tokens
let amount_in_after_fee = (amount_in as u128) * (10000 - fee_bps) / 10000;

// Sell: base_in → quote_out (token → SOL)
let quote_out = (quote_reserve as u128) * amount_in_after_fee
    / ((base_reserve as u128) + amount_in_after_fee);

// Buy: quote_in → base_out (SOL → token)
let base_out = (base_reserve as u128) * amount_in_after_fee
    / ((quote_reserve as u128) + amount_in_after_fee);
```

Set `fee_bps: 125` in the PumpSwap PoolState. This makes the simulator conservative — it only approves routes where profit exceeds 125 bps of fees. Routes on high-cap tokens will have lower actual fees, so the on-chain execution gets more output than estimated (never less).

## 5. Swap IX Builder

New function `build_pumpswap_swap_ix` in `bundle.rs`.

### Instruction Discriminators

| Direction | Discriminator |
|-----------|--------------|
| Buy (SOL → token) | `[102, 6, 61, 18, 1, 218, 235, 234]` |
| Buy ExactIn (SOL → token) | `[198, 46, 21, 82, 180, 217, 232, 112]` |
| Sell (token → SOL) | `[51, 230, 133, 164, 1, 127, 131, 173]` |

For arb, we use **Sell** (ExactIn) when selling token for SOL, and **Buy** when buying token with SOL.

### Sell Instruction Data

```rust
let mut data = Vec::with_capacity(24);
data.extend_from_slice(&PUMPSWAP_SELL_DISCRIMINATOR); // 8 bytes
data.extend_from_slice(&base_amount_in.to_le_bytes()); // u64
data.extend_from_slice(&min_quote_amount_out.to_le_bytes()); // u64
```

### Buy Instruction Data

```rust
let mut data = Vec::with_capacity(25);
data.extend_from_slice(&PUMPSWAP_BUY_DISCRIMINATOR); // 8 bytes
data.extend_from_slice(&base_amount_out.to_le_bytes()); // u64
data.extend_from_slice(&max_quote_amount_in.to_le_bytes()); // u64
data.push(0u8); // track_volume = None (OptionBool)
```

### CPI Account Order (Sell, 21 accounts)

| # | Account | Writable | Source |
|---|---------|----------|--------|
| 0 | pool | Yes | pool_address |
| 1 | user (signer) | Yes | signer_pubkey |
| 2 | global_config | No | `ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw` (hardcoded) |
| 3 | base_mint | No | from pool state |
| 4 | quote_mint (wSOL) | No | from pool state |
| 5 | user_base_token_account | Yes | derive_ata(signer, base_mint) |
| 6 | user_quote_token_account | Yes | derive_ata(signer, quote_mint) |
| 7 | pool_base_token_account | Yes | from pool state (vault_a) |
| 8 | pool_quote_token_account | Yes | from pool state (vault_b) |
| 9 | protocol_fee_recipient | No | round-robin from 8 addresses |
| 10 | protocol_fee_recipient_token_account | Yes | derive_ata(fee_recipient, quote_mint) |
| 11 | base_token_program | No | resolved from mint (SPL Token or Token-2022) |
| 12 | quote_token_program | No | SPL Token (wSOL is always SPL Token) |
| 13 | system_program | No | `11111111111111111111111111111111` |
| 14 | associated_token_program | No | `ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL` |
| 15 | event_authority | No | `GS4CU59F31iL7aR2Q8zVS8DRrcRnXX1yjQ66TqNVQnaR` (hardcoded) |
| 16 | pumpswap_program | No | `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA` |
| 17 | coin_creator_vault_ata | Yes | derive from coin_creator_vault_authority + quote_mint |
| 18 | coin_creator_vault_authority | No | PDA `["creator_vault", coin_creator]` on PumpSwap |
| 19 | fee_config | No | `5PHirr8joyTMp9JMm6nW7hNDVyEYdkzDqazxPD7RaTjx` (hardcoded) |
| 20 | fee_program | No | `pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ` (hardcoded) |

**Buy adds 2 more:** global_volume_accumulator (19) + user_volume_accumulator (20), shifting fee_config/fee_program to 21/22 = **23 accounts total**.

Note: pool_v2 is NOT part of the PumpSwap AMM IDL (it's from the Pump bonding curve program). Do not include it.

### Protocol Fee Recipients (round-robin)

8 hardcoded addresses, rotated per swap:
```
FWsW1xNtWscwNmKv6wVsU1iTzRN6wmmk3MjxRP5tT7hz
G5UZAVbAf46s7cKWoyKu8kYTip9DGTpbLZ2qa9Aq69dP
7hTckgnGnLQR6sdH7YkqFTAA7VwTfYFaZ6EhEsU3saCX
9rPYyANsfQZw3DnDmKE3YCQF5E8oD89UXoHn9JFEhJUz
7VtfL8fvgNfhz17qKRMjzQEXgbdpnHHHQRh54R9jP2RJ
AVmoTthdrX6tKt4nDjco2D775W2YK3sDhxPcMmzUAmTY
62qc2CNXwrYqQScmEdiZFFAnJR262PxWEuNQtxfafNgV
JCRGumoE9Qi5BBgULTgdgTLjSgkCMSbF62ZZfGs84JeU
```

And their corresponding wSOL ATAs (derived at build time, hardcoded for ALT).

## 6. ALT Expansion

Add these global PumpSwap accounts to the base ALT (21 addresses):

| Address | Role |
|---------|------|
| `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA` | PumpSwap Program |
| `ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw` | Global Config |
| `GS4CU59F31iL7aR2Q8zVS8DRrcRnXX1yjQ66TqNVQnaR` | Event Authority |
| `5PHirr8joyTMp9JMm6nW7hNDVyEYdkzDqazxPD7RaTjx` | Fee Config |
| `pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ` | Fee Program |
| `C2aFPdENg4A2HQsmrd5rTw5TaYBX5Ku887cWjbFKtZpw` | Global Volume Accumulator |
| 8 fee recipient addresses | Protocol Fee Recipients |
| 8 fee recipient wSOL ATAs | Fee Recipient Token Accounts |

Total ALT expansion: 33 current + 21 new = **54 addresses**.

## 7. Addresses Module

Add to `src/addresses.rs`:

```rust
pub const PUMPSWAP: Pubkey = /* pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA */;
pub const PUMPSWAP_GLOBAL_CONFIG: Pubkey = /* ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw */;
pub const PUMPSWAP_EVENT_AUTHORITY: Pubkey = /* GS4CU59F31iL7aR2Q8zVS8DRrcRnXX1yjQ66TqNVQnaR */;
pub const PUMPSWAP_FEE_CONFIG: Pubkey = /* 5PHirr8joyTMp9JMm6nW7hNDVyEYdkzDqazxPD7RaTjx */;
pub const PUMPSWAP_FEE_PROGRAM: Pubkey = /* pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ */;
pub const PUMPSWAP_GLOBAL_VOLUME_ACCUMULATOR: Pubkey = /* C2aFPdENg4A2HQsmrd5rTw5TaYBX5Ku887cWjbFKtZpw */;
```

## 8. Route Calculator + Submission

- Add `DexType::PumpSwap` to `can_submit_route()` in `router/mod.rs`
- The existing route calculator already finds cross-DEX routes via the token→pool index
- PumpSwap pools automatically participate because they share SOL and memecoin mints with other DEXes

## 9. Edge Cases

- **Token-2022 base mints:** Some Pump.fun tokens use Token-2022. The base_token_program must be resolved from mint account owner (same mint resolution cache as other DEXes).
- **Mayhem mode:** Uses special fee recipient `GesfTA3X2arioaHp8bbKdjG9vJtskViWACZoYvxp4twS`. Check `is_mayhem_mode` from pool state.
- **Cashback coins:** Add volume accumulator accounts. Check `is_cashback_coin` from pool state.
- **pool_v2 account:** Always required as last account. PDA `["pool-v2", base_mint]` on PumpSwap program.
- **CPI direction inversion:** `is_pump_swap_cpi_buy = !is_base_in`. When we sell token (base_in=true), the CPI uses the sell discriminator.

## Files Modified

| File | Change |
|------|--------|
| `src/addresses.rs` | Add PumpSwap const addresses |
| `src/router/pool.rs` | Add `DexType::PumpSwap`, add `coin_creator`/`is_mayhem_mode`/`is_cashback_coin` to PoolExtra |
| `src/router/mod.rs` | Add PumpSwap to `can_submit_route()` |
| `src/config.rs` | Add PumpSwap to `monitored_programs()` |
| `src/mempool/stream.rs` | Add `parse_pumpswap()`, Geyser routing by discriminator, vault fetch |
| `src/executor/bundle.rs` | Add `build_pumpswap_swap_ix()` |
| `src/bin/setup_alt.rs` | Add 21 PumpSwap addresses to ALT |
| `tests/unit/stream_parsing.rs` | TDD: PumpSwap parser tests |
| `tests/unit/bundle_real_ix.rs` | TDD: PumpSwap swap IX tests |
| `tests/unit/submission_filter.rs` | TDD: PumpSwap route accepted |
| `CLAUDE.md` | Update DEX table, module map |

## Testing

TDD for each component:
1. Parser: verify mints, vaults, creator extracted at correct offsets
2. Parser: reject invalid data sizes, wrong discriminator
3. Pricing: verify CPMM output with 30 bps fee matches expected
4. Swap IX: verify account count (20 for sell, 21+ for buy)
5. Swap IX: verify discriminator bytes
6. Swap IX: verify coin_creator_vault PDA derivation
7. Submission filter: PumpSwap routes accepted
8. E2e: route discovery with PumpSwap + Orca pools

## Non-Goals

- Cashback coin special handling (first pass ignores cashback — treats as normal sell)
- Mayhem mode fee recipient override (uses normal round-robin)
- Volume tracking (track_volume = false/None)
- Per-pool ALTs for PumpSwap (future optimization)
