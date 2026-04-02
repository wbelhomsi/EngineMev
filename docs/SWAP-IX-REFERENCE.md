# DEX Swap Instruction Reference

Accounts and PDA derivations for building raw swap instructions.
Extracted from a production trading system that uses a CPI router — the per-DEX accounts are the same for direct calls.

**Note:** Instruction discriminators are NOT included here (they differ between router CPI and raw calls). See individual sections for what's known.

---

## Raydium AMM v4 (`675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8`)

**Raw swap discriminator:** `[9]` (single byte, same for V1 and V2)

**Instruction data:** `discriminator (1B) + amount_in (u64 LE) + minimum_amount_out (u64 LE)` = 17 bytes

### Accounts (raw instruction, not router)

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

**Note:** This is the V1 layout (17 accounts). V2 reduces to 8 accounts by removing OpenBook market accounts. The router abstracts this — for raw calls, V2 is preferred when the pool supports it.

**Router shortcut:** The CPI Router only passes 5 per-swap accounts (pool, base_vault, quote_vault, output_ata, output_mint) and resolves OpenBook internally via CPI.

---

## Raydium CP (`CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C`)

**Raw swap discriminator:** Anchor `swap_base_input` — TBD (need to compute `sha256("global:swap_base_input")[..8]`)

**Instruction data:** `discriminator (8B) + amount_in (u64 LE) + minimum_amount_out (u64 LE)` = 24 bytes

### Accounts (raw instruction)

| # | Account | Signer | Writable | Source |
|---|---------|--------|----------|--------|
| 0 | Payer (signer) | yes | yes | Wallet |
| 1 | Authority | no | no | PDA of CPMM program |
| 2 | AMM Config | no | no | From pool state offset 8 |
| 3 | Pool State | no | yes | Pool account (637 bytes) |
| 4 | User Input Token | no | yes | User's ATA for input |
| 5 | User Output Token | no | yes | User's ATA for output |
| 6 | Input Vault | no | yes | From pool state: token_0_vault (72) or token_1_vault (104) |
| 7 | Output Vault | no | yes | The other vault |
| 8 | Input Token Program | no | no | SPL Token or Token-2022 (from pool state offset 232 or 264) |
| 9 | Output Token Program | no | no | The other token program |
| 10 | Input Token Mint | no | no | From pool state offset 168 or 200 |
| 11 | Output Token Mint | no | no | The other mint |
| 12 | Observation State | no | yes | PDA: `["observation", pool_id]` |

### PDA Derivations

```
Authority: seeds=[], program=CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C (standard Anchor PDA)
Observation: seeds=["observation", pool_id.to_bytes()], program=CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C
```

---

## Orca Whirlpool (`whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc`)

**Raw swap discriminator:** Anchor `swap` — TBD

**Instruction data:** `discriminator (8B) + amount (u64) + other_amount_threshold (u64) + sqrt_price_limit (u128) + amount_specified_is_input (bool) + a_to_b (bool)` = 42 bytes

### Accounts (raw instruction)

| # | Account | Signer | Writable | Source |
|---|---------|--------|----------|--------|
| 0 | Token Program | no | no | Static |
| 1 | Token Authority (signer) | yes | no | Wallet |
| 2 | Whirlpool | no | yes | Pool account |
| 3 | Token Owner Account A | no | yes | User's ATA for token A |
| 4 | Token Vault A | no | yes | From pool state offset 133 |
| 5 | Token Owner Account B | no | yes | User's ATA for token B |
| 6 | Token Vault B | no | yes | From pool state offset 213 |
| 7 | Tick Array 0 | no | yes | PDA (see below) |
| 8 | Tick Array 1 | no | yes | PDA |
| 9 | Tick Array 2 | no | yes | PDA |
| 10 | Oracle | no | yes | PDA: `["oracle", pool_id]` |

### Tick Array PDAs

```
seeds = ["tick_array", pool_id.to_bytes(), start_index.to_string().as_bytes()]
program = whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc
```

- Up to 3 tick arrays, each holds 88 ticks
- `start_index` is the string representation of the tick index (e.g., `"0"`, `"-7920"`)
- Selection based on current tick, tick_spacing, and swap direction

### Constants

- `MIN_SQRT_PRICE = 4295048016`
- `MAX_SQRT_PRICE = 79226673515401279992447579055`
- `TICK_ARRAY_SIZE = 88`
- `FEE_RATE_MUL_VALUE = 1,000,000`

---

## Meteora DLMM (`LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo`)

**Raw swap discriminator:** Anchor `swap` (v2 with memo) — TBD

### Accounts (raw instruction)

| # | Account | Signer | Writable | Source |
|---|---------|--------|----------|--------|
| 0 | LbPair | no | yes | Pool account (904 bytes) |
| 1 | Bin Array Bitmap Extension | no | yes | PDA (optional, if needed) |
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
| 13 | Event Authority | no | no | PDA |
| 14 | Program | no | no | DLMM program ID |
| 15..N | Bin Arrays (1-16) | no | yes | PDAs |

### Bin Array PDAs

```
seeds = ["bin_array", lb_pair_id.to_bytes(), index_as_i64_le]
program = LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo
```

- Up to 16 bin arrays, each holds 70 bins
- Bin array index for a `binId`: `floor(binId / 70)` (signed division)

### Other PDAs

```
Oracle: seeds=["oracle", lb_pair_id.to_bytes()]
Bitmap Extension: seeds=["bitmap", lb_pair_id.to_bytes()]
Event Authority: seeds=["__event_authority"]
```

---

## Meteora DAMM v2 (`cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG`)

**Raw swap discriminator:** TBD

### Accounts (raw instruction)

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
| 9 | Event Authority | no | no | PDA |
| 10 | Program | no | no | DAMM v2 program ID |
| 11 | Payer (signer) | yes | yes | Wallet |

---

## Raydium CLMM (`CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK`)

**Raw swap discriminator:** Anchor `swap_v2` — TBD

**Instruction data:** `discriminator (8B) + amount (u64) + other_amount_threshold (u64) + sqrt_price_limit_x64 (u128) + is_base_input (bool)` = 41 bytes

### Accounts (raw instruction)

| # | Account | Signer | Writable | Source |
|---|---------|--------|----------|--------|
| 0 | Payer (signer) | yes | no | Wallet |
| 1 | AMM Config | no | no | From pool state offset 9 |
| 2 | Pool State | no | yes | Pool account (1560 bytes) |
| 3 | Input Token Account | no | yes | User's ATA for input |
| 4 | Output Token Account | no | yes | User's ATA for output |
| 5 | Input Vault | no | yes | vault_0 (137) or vault_1 (169) |
| 6 | Output Vault | no | yes | The other vault |
| 7 | Observation State | no | yes | From pool state offset 201 (`observation_key`) |
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

- Up to 4 tick arrays (default), each holds 60 ticks
- Bitmap-accelerated search
- `index` encoded as i32 big-endian (4 bytes)

### Other PDAs

```
Bitmap Extension: seeds=["pool_tick_array_bitmap_extension", pool_id.to_bytes()]
```

### Constants

- `MIN_SQRT_PRICE_X64 = 4295048016`
- `MAX_SQRT_PRICE_X64 = 79226673521066979257578248091`
- `TICK_ARRAY_SIZE = 60`
- `TICK_ARRAY_BITMAP_SIZE = 512`
