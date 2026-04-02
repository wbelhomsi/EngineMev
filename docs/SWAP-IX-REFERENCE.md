# DEX Swap Instruction Reference

Accounts, PDA derivations, discriminators, and data layouts for building raw swap instructions.
Verified against a production CPI router's on-chain source code (`a production CPI router's on-chain source code`).

> The CPI discriminators below are the same bytes used in raw direct calls — the Anchor `global:` hash
> produces the same 8-byte prefix regardless of whether you call via CPI or direct instruction.

---

## Discriminator + Data Layout Summary

| DEX | Instruction | Discriminator (hex) | Data after discriminator |
|-----|-------------|--------------------|----|
| Raydium AMM v4 | swap (ExactIn) | `09` (1 byte) | amount_in(u64) + min_out(u64) = 17B total |
| Raydium CP | swap_base_input | `8fbe5adac41e33de` | amount_in(u64) + min_out(u64) = 24B total |
| Orca Whirlpool | swap_v2 | `2b04ed0b1ac91e62` | amount(u64) + threshold(u64) + sqrt_price_limit(u128) + is_exact_in(bool) + a_to_b(bool) + remaining(Option) = 43B |
| Meteora DLMM v2 | swap2 | `414b3f4ceb5b5b88` | amount_in(u64) + min_out(u64) + remaining_accounts_info = 24B+ |
| Meteora DAMM v2 | swap2 | `414b3f4ceb5b5b88` | amount_0(u64) + amount_1(u64) + swap_mode(u8) = 25B |
| Raydium CLMM | swap_v2 | `2b04ed0b1ac91e62` | amount(u64) + threshold(u64) + sqrt_price_limit_x64(u128) + is_exact_in(bool) = 41B |

> Orca Whirlpool and Raydium CLMM share the same discriminator (`2b04ed0b1ac91e62`) — disambiguated by program ID.
> Meteora DLMM v2 and DAMM v2 share the same discriminator (`414b3f4ceb5b5b88`) — disambiguated by program ID.

---

## Raydium AMM v4 (`675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8`)

**Discriminator:** `[9]` (single byte, ExactIn). ExactOut = `[11]`.

**Data layout (ExactIn):**
| Offset | Size | Field |
|--------|------|-------|
| 0 | 1 | instruction: `9` |
| 1 | 8 | amount_in (u64 LE) |
| 9 | 8 | minimum_amount_out (u64 LE) |

### Accounts (raw V1 instruction — 17 accounts)

| # | Account | Signer | Writable | Source |
|---|---------|--------|----------|--------|
| 0 | Token Program | no | no | `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA` |
| 1 | AmmInfo (pool) | no | yes | Pool account (752 bytes) |
| 2 | Amm Authority | no | no | PDA of Raydium AMM program |
| 3 | Amm Open Orders | no | yes | From pool state offset 496 |
| 4 | Base Vault | no | yes | From pool state offset 336 |
| 5 | Quote Vault | no | yes | From pool state offset 368 |
| 6 | Market Program | no | no | From pool state offset 560 (OpenBook) |
| 7 | Market | no | yes | From pool state offset 528 |
| 8 | Market Bids | no | yes | Derived from market account |
| 9 | Market Asks | no | yes | Derived from market account |
| 10 | Market Event Queue | no | yes | Derived from market account |
| 11 | Market Base Vault | no | yes | Derived from market account |
| 12 | Market Quote Vault | no | yes | Derived from market account |
| 13 | Market Vault Signer | no | no | Derived from market account |
| 14 | User Source Token | no | yes | User's ATA for input token |
| 15 | User Dest Token | no | yes | User's ATA for output token |
| 16 | User (signer) | yes | no | Wallet |

**Notes:**
- V2 swap reduces to 8 accounts (no OpenBook). V1 still works everywhere.
- Does NOT support Token-2022.
- OpenBook accounts (market bids/asks/event_queue/vaults/signer) must be derived from the market account data.

---

## Raydium CP (`CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C`)

**Discriminator:** `[0x8f, 0xbe, 0x5a, 0xda, 0xc4, 0x1e, 0x33, 0xde]` (`swap_base_input`)

**Data layout:**
| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | discriminator |
| 8 | 8 | amount_in (u64 LE) |
| 16 | 8 | minimum_amount_out (u64 LE) |

### Accounts (raw instruction — 13 accounts)

| # | Account | Signer | Writable | Source |
|---|---------|--------|----------|--------|
| 0 | Payer (signer) | yes | yes | Wallet |
| 1 | Authority | no | no | PDA: `seeds=[], program=CPMM` |
| 2 | AMM Config | no | no | From pool state offset 8 |
| 3 | Pool State | no | yes | Pool account (637 bytes) |
| 4 | User Input Token | no | yes | User's ATA for input |
| 5 | User Output Token | no | yes | User's ATA for output |
| 6 | Input Vault | no | yes | token_0_vault (72) or token_1_vault (104) |
| 7 | Output Vault | no | yes | The other vault |
| 8 | Input Token Program | no | no | SPL Token or Token-2022 (from offset 232 or 264) |
| 9 | Output Token Program | no | no | The other token program |
| 10 | Input Token Mint | no | no | From pool state offset 168 or 200 |
| 11 | Output Token Mint | no | no | The other mint |
| 12 | Observation State | no | yes | PDA: `["observation", pool_id]` |

### PDAs

```
Authority: seeds=[], program=CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C
Observation: seeds=["observation", pool_id.to_bytes()], program=CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C
```

**Supports Token-2022.** Each pool stores its token programs at offsets 232 and 264.

---

## Orca Whirlpool (`whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc`)

**Discriminator:** `[0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62]` (`swap_v2`)

**Data layout:**
| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | discriminator |
| 8 | 8 | amount (u64 LE) |
| 16 | 8 | other_amount_threshold (u64 LE) |
| 24 | 16 | sqrt_price_limit (u128 LE) — use `0` for no limit |
| 40 | 1 | is_exact_in (bool) — `true` for ExactIn |
| 41 | 1 | a_to_b (bool) |
| 42 | 1 | remaining_accounts_info: `0` = None |

### Accounts (raw instruction — 11 accounts + tick arrays)

| # | Account | Signer | Writable | Source |
|---|---------|--------|----------|--------|
| 0 | Token Program | no | no | Static |
| 1 | Token Authority (signer) | yes | no | Wallet |
| 2 | Whirlpool | no | yes | Pool account (653 bytes) |
| 3 | Token Owner Account A | no | yes | User's ATA for token A |
| 4 | Token Vault A | no | yes | From pool state offset 133 |
| 5 | Token Owner Account B | no | yes | User's ATA for token B |
| 6 | Token Vault B | no | yes | From pool state offset 213 |
| 7 | Tick Array 0 | no | yes | PDA |
| 8 | Tick Array 1 | no | yes | PDA |
| 9 | Tick Array 2 | no | yes | PDA |
| 10 | Oracle | no | yes | PDA: `["oracle", pool_id]` |

### Tick Array PDAs

```
seeds = ["tick_array", pool_id.to_bytes(), start_index.to_string().as_bytes()]
program = whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc
```

- Max 3 tick arrays, 88 ticks each
- `start_index` as string (e.g., `"0"`, `"-7920"`)
- Selection from swap simulation output

### Constants

```
MIN_SQRT_PRICE = 4295048016
MAX_SQRT_PRICE = 79226673515401279992447579055
TICK_ARRAY_SIZE = 88
FEE_RATE_MUL_VALUE = 1_000_000
```

---

## Meteora DLMM (`LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo`)

**Discriminator:** `[0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88]` (`swap2`, v2 with memo)

Legacy `swap` discriminator: `[0xf8, 0xc6, 0x9e, 0x91, 0xe1, 0x75, 0x87, 0xc8]`

**Data layout (swap2):**
| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | discriminator |
| 8 | 8 | amount_in (u64 LE) |
| 16 | 8 | min_amount_out (u64 LE) |
| 24 | 4+ | remaining_accounts_info_slices (Vec<(u8, u8)>) |

### Accounts (raw instruction — 15+ accounts)

| # | Account | Signer | Writable | Source |
|---|---------|--------|----------|--------|
| 0 | LbPair | no | yes | Pool account (904 bytes) |
| 1 | Bin Array Bitmap Extension | no | yes | PDA (optional) |
| 2 | Reserve X (Vault) | no | yes | From pool state offset 152 |
| 3 | Reserve Y (Vault) | no | yes | From pool state offset 184 |
| 4 | User Token In | no | yes | User's ATA for input |
| 5 | User Token Out | no | yes | User's ATA for output |
| 6 | Token X Mint | no | no | From pool state offset 88 |
| 7 | Token Y Mint | no | no | From pool state offset 120 |
| 8 | Oracle | no | yes | PDA: `["oracle", lb_pair_id]` |
| 9 | Host Fee In | no | yes | (optional) |
| 10 | User (signer) | yes | no | Wallet |
| 11 | Token X Program | no | no | SPL Token |
| 12 | Token Y Program | no | no | SPL Token |
| 13 | Event Authority | no | no | PDA: `["__event_authority"]` |
| 14 | Program | no | no | DLMM program ID |
| 15..N | Bin Arrays (1-16) | no | yes | PDAs |

### Bin Array PDAs

```
seeds = ["bin_array", lb_pair_id.to_bytes(), index_as_i64_le]
program = LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo
```

- Max 16 bin arrays, 70 bins each
- Bin array index: `floor(binId / 70)` (signed)

### Other PDAs

```
Oracle: seeds=["oracle", lb_pair_id.to_bytes()]
Bitmap Extension: seeds=["bitmap", lb_pair_id.to_bytes()]
Event Authority: seeds=["__event_authority"], program=DLMM
```

---

## Meteora DAMM v2 (`cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG`)

**Discriminator:** `[0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88]` (`swap2`)

**Data layout:**
| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | discriminator |
| 8 | 8 | amount_0 (u64 LE) — input amount for ExactIn |
| 16 | 8 | amount_1 (u64 LE) — min output for ExactIn |
| 24 | 1 | swap_mode (u8) — 0=ExactIn, 1=ExactOut |

### Accounts (raw instruction — 11+ accounts)

| # | Account | Signer | Writable | Source |
|---|---------|--------|----------|--------|
| 0 | Pool State | no | yes | Pool account (1112 bytes) |
| 1 | Pool Authority | no | no | PDA |
| 2 | Input Vault | no | yes | token_a_vault (232) or token_b_vault (264) |
| 3 | Output Vault | no | yes | The other vault |
| 4 | User Input Token | no | yes | User's ATA for input |
| 5 | User Output Token | no | yes | User's ATA for output |
| 6 | Input Mint | no | no | |
| 7 | Output Mint | no | no | |
| 8 | Token Program | no | no | SPL Token |
| 9 | Event Authority | no | no | PDA: `["__event_authority"]` |
| 10 | Program | no | no | DAMM v2 program ID |
| 11 | Payer (signer) | yes | yes | Wallet |

**Note:** Requires Instruction Sysvar account in the transaction.

---

## Raydium CLMM (`CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK`)

**Discriminator:** `[0x2b, 0x04, 0xed, 0x0b, 0x1a, 0xc9, 0x1e, 0x62]` (`swap_v2`)

**Data layout:**
| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | discriminator |
| 8 | 8 | amount (u64 LE) |
| 16 | 8 | other_amount_threshold (u64 LE) — min_out for ExactIn |
| 24 | 16 | sqrt_price_limit_x64 (u128 LE) — use `0` for no limit |
| 40 | 1 | is_exact_in (bool) |

### Accounts (raw instruction — 14+ accounts)

| # | Account | Signer | Writable | Source |
|---|---------|--------|----------|--------|
| 0 | Payer (signer) | yes | no | Wallet |
| 1 | AMM Config | no | no | From pool state offset 9 |
| 2 | Pool State | no | yes | Pool account (1560 bytes) |
| 3 | Input Token Account | no | yes | User's ATA for input |
| 4 | Output Token Account | no | yes | User's ATA for output |
| 5 | Input Vault | no | yes | vault_0 (137) or vault_1 (169) |
| 6 | Output Vault | no | yes | The other vault |
| 7 | Observation State | no | yes | From pool state offset 201 |
| 8 | Token Program (input) | no | no | SPL Token or Token-2022 |
| 9 | Token Program (output) | no | no | |
| 10 | Memo Program | no | no | |
| 11 | Input Mint | no | no | |
| 12 | Output Mint | no | no | |
| 13 | Bitmap Extension | no | yes | PDA: `["pool_tick_array_bitmap_extension", pool_id]` |
| 14..N | Tick Arrays (1-4) | no | yes | PDAs |

### Tick Array PDAs

```
seeds = ["tick_array", pool_id.to_bytes(), index_as_i32_be]
program = CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK
```

- Default 4 tick arrays, 60 ticks each
- Bitmap-accelerated search
- Index as i32 big-endian

### Other PDAs

```
Bitmap Extension: seeds=["pool_tick_array_bitmap_extension", pool_id.to_bytes()]
Observation: read from pool state offset 201 (observationKey field)
```

### Constants

```
MIN_SQRT_PRICE_X64 = 4295048016
MAX_SQRT_PRICE_X64 = 79226673521066979257578248091
TICK_ARRAY_SIZE = 60
TICK_ARRAY_BITMAP_SIZE = 512
```
