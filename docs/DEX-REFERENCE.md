# DEX Reference — Solana Pool Account Layouts & Quoting Math

Authoritative reference for all supported DEX programs. Account offsets verified against production trading systems and on-chain source code (April 2026).

---

## Supported Programs

| DEX | Program ID | Account Size | Anchor? | Reserves Source |
|-----|-----------|-------------|---------|----------------|
| Raydium AMM v4 | `675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8` | 752 | No | Vault balances (SPL Token accounts) |
| Raydium CP (CPMM) | `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C` | 637 | Yes | Vault balances minus fee accumulators |
| Raydium CLMM | `CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK` | 1560 | Yes | sqrt_price + tick + liquidity (CLMM math) |
| Orca Whirlpool | `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc` | 653 | Yes | sqrt_price + tick + liquidity (CLMM math) |
| Meteora DLMM | `LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo` | 904 | Yes | Bin-by-bin simulation (bin arrays) |
| Meteora DAMM v2 | `cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG` | 1112 | Yes | Pool state (dual mode: CP or concentrated) |
| Sanctum Infinity | `5ocnV1qiCgaQR8Jb8xWnVbApfaygJ8tNoZfgPwsgx9kx` | varies | Yes | Virtual pool (synthetic reserves from oracle rate) |

---

## Account Layouts

### Raydium AMM v4 — AmmInfo (752 bytes, no discriminator)

No Anchor discriminator. Data starts at byte 0. First 24 fields are sequential u64s.

| Offset | Field | Type | Size | Notes |
|--------|-------|------|------|-------|
| 0 | status | u64 | 8 | 6 = active (SwapOnly) |
| 8 | nonce | u64 | 8 | |
| 16 | maxOrder | u64 | 8 | |
| 24 | depth | u64 | 8 | |
| 32 | baseDecimal | u64 | 8 | Token A decimals |
| 40 | quoteDecimal | u64 | 8 | Token B decimals |
| 48 | state | u64 | 8 | |
| 56 | resetFlag | u64 | 8 | |
| 64 | minSize | u64 | 8 | |
| 72 | volMaxCutRatio | u64 | 8 | |
| 80 | amountWaveRatio | u64 | 8 | |
| 88 | baseLotSize | u64 | 8 | |
| 96 | quoteLotSize | u64 | 8 | |
| 104 | minPriceMultiplier | u64 | 8 | |
| 112 | maxPriceMultiplier | u64 | 8 | |
| 120 | systemDecimalValue | u64 | 8 | |
| 128 | minSeparateNumerator | u64 | 8 | |
| 136 | minSeparateDenominator | u64 | 8 | |
| 144 | tradeFeeNumerator | u64 | 8 | |
| 152 | tradeFeeDenominator | u64 | 8 | |
| 160 | pnlNumerator | u64 | 8 | |
| 168 | pnlDenominator | u64 | 8 | |
| 176 | swapFeeNumerator | u64 | 8 | |
| 184 | swapFeeDenominator | u64 | 8 | |
| 192 | baseNeedTakePnl | u64 | 8 | Not used for quoting |
| 200 | quoteNeedTakePnl | u64 | 8 | Not used for quoting |
| 208-255 | ... | | | Swap accumulators (u128s) + fees |
| 256 | swapBaseInAmount | u128 | 16 | |
| 272 | swapQuoteOutAmount | u128 | 16 | |
| 288 | swapQuoteInAmount | u128 | 16 | |
| 304 | swapBaseOutAmount | u128 | 16 | |
| ... | (fee fields) | | | |
| **336** | **baseVault** | **Pubkey** | **32** | SPL Token vault for token A |
| **368** | **quoteVault** | **Pubkey** | **32** | SPL Token vault for token B |
| **400** | **baseMint** | **Pubkey** | **32** | Token A mint |
| **432** | **quoteMint** | **Pubkey** | **32** | Token B mint |
| 464 | lpMint | Pubkey | 32 | |
| 496 | openOrders | Pubkey | 32 | OpenBook market |
| 528 | marketId | Pubkey | 32 | |
| 560 | marketProgramId | Pubkey | 32 | |
| 592 | targetOrders | Pubkey | 32 | |
| 624 | withdrawQueue | Pubkey | 32 | |
| 656 | lpVault | Pubkey | 32 | |
| 688 | owner | Pubkey | 32 | |
| 720 | lpReserve | u64 | 8 | |
| 728 | padding | [u64; 3] | 24 | |

**Quoting:** Pure constant product on raw vault balances. `output = (reserve_out * input) / (reserve_in + input)` with fee applied.

**Fee:** `tradeFeeNumerator / tradeFeeDenominator` (offsets 144/152).

**Status filter for getProgramAccounts:** `memcmp` at offset 0, base64 `BgAAAAAAAAA=` (u64 LE value 6).

---

### Raydium CP — PoolState (637 bytes, Anchor)

Anchor discriminator: `[247, 237, 227, 245, 215, 195, 222, 70]`

`#[repr(C, packed)]` — no padding between fields. u64s at odd offsets require unaligned reads.

| Offset | Field | Type | Size |
|--------|-------|------|------|
| 0 | discriminator | [u8; 8] | 8 |
| 8 | amm_config | Pubkey | 32 |
| 40 | pool_creator | Pubkey | 32 |
| **72** | **token_0_vault** | **Pubkey** | **32** |
| **104** | **token_1_vault** | **Pubkey** | **32** |
| 136 | lp_mint | Pubkey | 32 |
| **168** | **token_0_mint** | **Pubkey** | **32** |
| **200** | **token_1_mint** | **Pubkey** | **32** |
| 232 | token_0_program | Pubkey | 32 |
| 264 | token_1_program | Pubkey | 32 |
| 296 | observation_key | Pubkey | 32 |
| 328 | auth_bump | u8 | 1 |
| 329 | status | u8 | 1 | Bit 2 (value 4) = swap disabled |
| 330 | lp_mint_decimals | u8 | 1 |
| 331 | mint_0_decimals | u8 | 1 |
| 332 | mint_1_decimals | u8 | 1 |
| 333 | lp_supply | u64 | 8 |
| **341** | **protocol_fees_token_0** | **u64** | **8** |
| **349** | **protocol_fees_token_1** | **u64** | **8** |
| **357** | **fund_fees_token_0** | **u64** | **8** |
| **365** | **fund_fees_token_1** | **u64** | **8** |
| 373 | open_time | u64 | 8 |
| 381 | recent_epoch | u64 | 8 |
| 389 | creator_fee_on | u8 | 1 |
| 390 | enable_creator_fee | bool | 1 |
| 391 | padding1 | [u8; 6] | 6 |
| **397** | **creator_fees_token_0** | **u64** | **8** |
| **405** | **creator_fees_token_1** | **u64** | **8** |
| 413 | padding | [u64; 28] | 224 |

**Quoting:**
```
effectiveReserve0 = max(vaultBalance0 - protocolFees0 - fundFees0 - creatorFees0, 0)
effectiveReserve1 = max(vaultBalance1 - protocolFees1 - fundFees1 - creatorFees1, 0)
output = (effectiveReserve1 * input) / (effectiveReserve0 + input)
```
Fee applied before the constant product swap (fee comes from amm_config account, not stored in pool state).

**Supports Token-2022:** `token_0_program` and `token_1_program` (offsets 232, 264) identify which token program each side uses.

---

### Raydium CLMM — PoolState (1560 bytes, Anchor, packed)

`#[repr(C, packed)]` — no padding.

| Offset | Field | Type | Size |
|--------|-------|------|------|
| 0 | discriminator | [u8; 8] | 8 |
| 8 | bump | [u8; 1] | 1 |
| 9 | amm_config | Pubkey | 32 |
| 41 | owner | Pubkey | 32 |
| **73** | **token_mint_0** | **Pubkey** | **32** |
| **105** | **token_mint_1** | **Pubkey** | **32** |
| **137** | **token_vault_0** | **Pubkey** | **32** |
| **169** | **token_vault_1** | **Pubkey** | **32** |
| 201 | observation_key | Pubkey | 32 |
| 233 | mint_decimals_0 | u8 | 1 |
| 234 | mint_decimals_1 | u8 | 1 |
| 235 | tick_spacing | u16 | 2 |
| **237** | **liquidity** | **u128** | **16** |
| **253** | **sqrt_price_x64** | **u128** | **16** |
| **269** | **tick_current** | **i32** | **4** |
| 273 | padding3 | u16 | 2 |
| 275 | padding4 | u16 | 2 |
| 277 | fee_growth_global_0_x64 | u128 | 16 |
| 293 | fee_growth_global_1_x64 | u128 | 16 |
| 309 | protocol_fees_token_0 | u64 | 8 |
| 317 | protocol_fees_token_1 | u64 | 8 |
| ... | (swap amounts, rewards, tick arrays, padding) | | |
| 389 | status | u8 | 1 |

**Quoting:** Tick-crossing CLMM math (see CLMM Math section below). Requires tick array accounts. 60 ticks per array, bitmap-accelerated search.

---

### Orca Whirlpool — Whirlpool (653 bytes, Anchor)

Borsh-serialized (NOT `repr(C)`). No alignment padding. Fields at odd offsets are normal.

| Offset | Field | Type | Size |
|--------|-------|------|------|
| 0 | discriminator | [u8; 8] | 8 |
| 8 | whirlpools_config | Pubkey | 32 |
| 40 | whirlpool_bump | u8 | 1 |
| 41 | tick_spacing | u16 | 2 |
| 43 | tick_spacing_seed | [u8; 2] | 2 |
| 45 | fee_rate | u16 | 2 |
| 47 | protocol_fee_rate | u16 | 2 |
| **49** | **liquidity** | **u128** | **16** |
| **65** | **sqrt_price** | **u128** | **16** |
| **81** | **tick_current_index** | **i32** | **4** |
| 85 | protocol_fee_owed_a | u64 | 8 |
| 93 | protocol_fee_owed_b | u64 | 8 |
| **101** | **token_mint_a** | **Pubkey** | **32** |
| **133** | **token_vault_a** | **Pubkey** | **32** |
| 165 | fee_growth_global_a | u128 | 16 |
| **181** | **token_mint_b** | **Pubkey** | **32** |
| **213** | **token_vault_b** | **Pubkey** | **32** |
| 245 | fee_growth_global_b | u128 | 16 |
| 261 | reward_last_updated_timestamp | u64 | 8 |
| 269 | reward_infos | [WhirlpoolRewardInfo; 3] | 384 |

**Quoting:** Tick-crossing CLMM math (see CLMM Math section below). Requires tick array accounts. 88 ticks per array, linear search.

---

### Meteora DLMM — LbPair (904 bytes, Anchor)

| Offset | Field | Type | Size |
|--------|-------|------|------|
| 0 | discriminator | [u8; 8] | 8 |
| 8 | parameters (StaticParameters) | struct | 30 |
| 38 | v_parameters (VariableParameters) | struct | 32 |
| 70 | bump_seed | [u8; 1] | 1 |
| 71 | bin_step_seed | [u8; 2] | 2 |
| 73 | pair_type | u8 | 1 |
| **76** | **active_id** | **i32** | **4** |
| **80** | **bin_step** | **u16** | **2** |
| 82 | status | u8 | 1 |
| ... | | | |
| **88** | **token_x_mint** | **Pubkey** | **32** |
| **120** | **token_y_mint** | **Pubkey** | **32** |
| **152** | **reserve_x (vault)** | **Pubkey** | **32** |
| **184** | **reserve_y (vault)** | **Pubkey** | **32** |
| 216 | protocol_fee | struct | 16 |
| ... | (reward_infos, oracle, bitmap, padding) | | |

**Quoting:** Bin-by-bin simulation (see DLMM Bin Math section below). Requires bin array accounts (separate from pool state). Dynamic fees per bin.

---

### Meteora DAMM v2 — Pool (1112 bytes, Anchor)

Discriminator: `[241, 154, 109, 4, 17, 177, 109, 188]`

| Offset | Field | Type | Size |
|--------|-------|------|------|
| 0 | discriminator | [u8; 8] | 8 |
| 8 | pool_fees | PoolFeesStruct | 160 |
| **168** | **token_a_mint** | **Pubkey** | **32** |
| **200** | **token_b_mint** | **Pubkey** | **32** |
| **232** | **token_a_vault** | **Pubkey** | **32** |
| **264** | **token_b_vault** | **Pubkey** | **32** |
| 296 | whitelisted_vault | Pubkey | 32 |
| 328 | padding_0 | [u8; 32] | 32 |
| **360** | **liquidity** | **u128** | **16** |
| 376 | padding_1 | u128 | 16 |
| 392 | protocol_a_fee | u64 | 8 |
| 400 | protocol_b_fee | u64 | 8 |
| 408 | padding_2 | u128 | 16 |
| **424** | **sqrt_min_price** | **u128** | **16** |
| **440** | **sqrt_max_price** | **u128** | **16** |
| **456** | **sqrt_price** | **u128** | **16** |
| 472 | activation_point | u64 | 8 |
| 480 | activation_type | u8 | 1 |
| **481** | **pool_status** | **u8** | **1** |
| 482 | token_a_flag | u8 | 1 | Token-2022 flag |
| 483 | token_b_flag | u8 | 1 | Token-2022 flag |
| **484** | **collect_fee_mode** | **u8** | **1** | Determines quoting mode |
| 485 | pool_type | u8 | 1 |
| ... | (fees, metrics, creator, padding) | | |
| 648 | creator | Pubkey | 32 |
| **680** | **token_a_amount** | **u64** | **8** | Direct reserve A |
| **688** | **token_b_amount** | **u64** | **8** | Direct reserve B |
| 696 | layout_version | u8 | 1 |
| ... | (padding, reward_infos) | | |

**Two quoting modes based on `collectFeeMode` (offset 484):**

**Mode 4 (Compounding pools):**
```
reserve_a = token_a_amount (offset 680)
reserve_b = token_b_amount (offset 688)
output = reserve_b * input / (reserve_a + input)
```

**Mode 0-3 (Concentrated liquidity pools):**
Uses `liquidity` (360), `sqrtPrice` (456), `sqrtMinPrice` (424), `sqrtMaxPrice` (440).
Dynamic CLMM math — `getNextSqrtPrice` + liquidity delta calculations. No vault balances needed.

---

## Quoting Math

### Constant Product (Raydium AMM v4, Raydium CP, DAMM v2 mode 4)

```
output = (reserve_out * amount_in) / (reserve_in + amount_in)
```

Fee applied to `amount_in` before the swap:
```
amount_in_after_fee = amount_in * (fee_denominator - fee_numerator) / fee_denominator
```

### CLMM Fee Model (Orca Whirlpool, Raydium CLMM)

**IMPORTANT: CLMM fee denominator is 1,000,000, NOT 10,000 (basis points).**
A pool with 0.3% fee has `feeRate = 3000`. Fee is applied to the INPUT:
```
input_after_fee = input * (1_000_000 - feeRate) / 1_000_000
```
If you store fees as `fee_bps` (basis points), convert: `feeRate = fee_bps * 100`.

**Do NOT use f64 for CLMM math.** The `P * P_new` product of two Q64.64 values exceeds f64 precision. Use u128 with careful division ordering, or u256 for the b_to_a path.

### CLMM Tick-Crossing Math (Orca Whirlpool, Raydium CLMM)

All use Q64.64 fixed-point sqrt prices.

**Token amount formulas between two sqrt prices:**
```
deltaA = (liquidity × (sqrtPriceUpper - sqrtPriceLower)) << 64 / (sqrtPriceLower × sqrtPriceUpper)
deltaB = liquidity × (sqrtPriceUpper - sqrtPriceLower) >> 64
```

**Next sqrt price from input:**
```
// Token A input (price moves down):
nextSqrtPrice = (liquidity × sqrtPrice << 64) / (liquidity << 64 + amount × sqrtPrice)

// Token B input (price moves up):
nextSqrtPrice = sqrtPrice + (amount << 64) / liquidity
```

**Swap loop:**
1. Find next initialized tick in direction of swap
2. Compute max amount consumable to reach that tick
3. If input < max: partial fill, compute final sqrt_price, stop
4. If input >= max: consume full step, update liquidity by `liquidityNet` from tick, move to next tick, repeat

**Tick arrays:**
- Orca Whirlpool: 88 ticks per array, linear search
- Raydium CLMM: 60 ticks per array, bitmap-accelerated search

**Tick-to-sqrt-price conversion:** Bitwise decomposition — precomputed constants for each bit of the tick index representing powers of `sqrt(1.0001)`.

**Note:** Accurate multi-tick simulation requires tick array account data (separate accounts, not in pool state). For route discovery, single-tick CLMM math is used: compute output within the current tick's liquidity range. This underestimates output for large trades (conservative) but eliminates the false positives from constant-product approximation on synthetic reserves.

### DLMM Bin-by-Bin Math (Meteora DLMM)

**Price per bin (reference only — DO NOT compute at swap time):**
```
price = (1 + binStep / 10000) ^ binId
```
**WARNING:** This overflows for real bin IDs (max ~443636). Use arbitrary-precision math if needed.
At swap time, use the **precomputed `bin.price` (u128)** stored in the on-chain bin array accounts.
Each bin stores `{ amountX: i64, amountY: i64, price: u128 }`. Price is stored as U128 with 64-bit scale.

**Per-bin swap (X for Y):**
```
outAmount = (inAmount × bin.price) >> 64
maxAmountIn = ceil((bin.amountY << 64) / bin.price)
```

**Per-bin swap (Y for X):**
```
outAmount = (inAmount << 64) / bin.price
maxAmountIn = ceil((bin.amountX × bin.price) >> 64)
```

**Dynamic fees per bin:**
```
baseFee = baseFactor × binStep × 10 × 10^baseFeePowerFactor
variableFee = ceil(variableFeeControl × (volatilityAccumulator × binStep)² / 10^11)
totalFee = min(baseFee + variableFee, 10^8)
feeOnAmount = ceil((amount × totalFee) / (10^9 - totalFee))
```
Note: Both variable fee and fee-on-amount use ceiling division (round up).

**Swap loop:**
1. Check if current bin has liquidity
2. Compute max consumable at this bin
3. If input exceeds bin capacity: drain it, subtract, move `activeId ±1`, find next bin with liquidity via bitmap
4. Repeat until input exhausted

**Requires bin array accounts** (separate from pool state).

---

## SPL Token Account Layout

All vault accounts are SPL Token accounts (dataSize: 165 bytes):

| Offset | Field | Type | Size |
|--------|-------|------|------|
| 0 | mint | Pubkey | 32 |
| 32 | owner | Pubkey | 32 |
| **64** | **amount** | **u64** | **8** | The vault balance |
| 72 | delegate | COption<Pubkey> | 36 |
| 108 | state | u8 | 1 |
| 109 | is_native | COption<u64> | 12 |
| 121 | delegated_amount | u64 | 8 |
| 129 | close_authority | COption<Pubkey> | 36 |

For lazy vault fetch, use `dataSlice: { offset: 64, length: 8 }` to retrieve only the balance (8 bytes).

---

## Geyser Subscription Strategy

**Subscribe by DEX program owner** (not by vault accounts):
- Accounts owned by DEX programs are pool state accounts
- When a swap happens, the pool state account gets modified
- Parse per-DEX layout to extract reserves/pricing data
- For Raydium AMM v4 and CP: pool state change triggers lazy vault balance fetch

**Do NOT subscribe to Token Program** — would receive every token transfer on Solana (millions/sec).

**Do NOT subscribe to individual vault pubkeys** — provider limits prevent subscribing to >10-100 specific accounts.

---

## getProgramAccounts Filters

| DEX | dataSize | memcmp | Notes |
|-----|---------|--------|-------|
| Raydium AMM v4 | 752 | offset 0, base64 `BgAAAAAAAAA=` (status=6) | Active pools only |
| Raydium CP | 637 | — | Discriminator-based if needed |
| Raydium CLMM | 1560 | — | |
| Orca Whirlpool | 653 | — | |
| Meteora DLMM | 904 | — | |
| Meteora DAMM v2 | 1112 | — | Discriminator: base64 `8ZptBBGxbbw=` |
