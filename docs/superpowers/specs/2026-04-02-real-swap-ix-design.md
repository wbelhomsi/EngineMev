# Real Swap Instructions — Raydium CP + Meteora DAMM v2

## Goal

Replace placeholder swap instruction builders with real ones for Raydium CP and Meteora DAMM v2. Add dedup. Enable the first live bundle submissions.

## Scope

- **In scope:** Raydium CP and Meteora DAMM v2 swap IX builders, dedup, cache access in bundle builder, route filtering (only submit routes where all hops have real IX builders)
- **Out of scope:** Raydium AMM v4, Orca Whirlpool, Raydium CLMM, Meteora DLMM (need tick/bin array data — follow-up)

## Raydium CP Swap Instruction

**Program:** `CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C`
**Discriminator:** `[0x8f, 0xbe, 0x5a, 0xda, 0xc4, 0x1e, 0x33, 0xde]`

**Data:** 24 bytes
| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | discriminator |
| 8 | 8 | amount_in (u64 LE) |
| 16 | 8 | minimum_amount_out (u64 LE) |

**Accounts (13):**
| # | Account | Signer | Writable | Source |
|---|---------|--------|----------|--------|
| 0 | Payer | yes | yes | searcher_keypair |
| 1 | Authority | no | no | PDA: seeds=[], program=CP |
| 2 | AMM Config | no | no | Pool state offset 8 |
| 3 | Pool State | no | yes | pool_address |
| 4 | User Input Token | no | yes | ATA(searcher, input_mint) |
| 5 | User Output Token | no | yes | ATA(searcher, output_mint) |
| 6 | Input Vault | no | yes | token_0_vault (72) or token_1_vault (104) based on direction |
| 7 | Output Vault | no | yes | The other vault |
| 8 | Input Token Program | no | no | token_0_program (232) or token_1_program (264) |
| 9 | Output Token Program | no | no | The other |
| 10 | Input Token Mint | no | no | input_mint |
| 11 | Output Token Mint | no | no | output_mint |
| 12 | Observation State | no | yes | PDA: seeds=["observation", pool_id], program=CP |

**Direction logic:** If `input_mint == token_0_mint` (offset 168), then input vault = token_0_vault (72), output vault = token_1_vault (104). Otherwise reversed.

**Extra pool state fields needed:** amm_config (offset 8, Pubkey), token_0_program (offset 232, Pubkey), token_1_program (offset 264, Pubkey). These must be parsed from the Geyser event and stored.

## Meteora DAMM v2 Swap Instruction

**Program:** `cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG`
**Discriminator:** `[0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88]`

**Data:** 25 bytes
| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | discriminator |
| 8 | 8 | amount_in (u64 LE) |
| 16 | 8 | minimum_amount_out (u64 LE) |
| 24 | 1 | swap_mode: 0 = ExactIn |

**Accounts (12):**
| # | Account | Signer | Writable | Source |
|---|---------|--------|----------|--------|
| 0 | Pool State | no | yes | pool_address |
| 1 | Pool Authority | no | no | PDA: seeds=[], program=DAMM_v2 |
| 2 | Input Vault | no | yes | token_a_vault (232) or token_b_vault (264) |
| 3 | Output Vault | no | yes | The other vault |
| 4 | User Input Token | no | yes | ATA(searcher, input_mint) |
| 5 | User Output Token | no | yes | ATA(searcher, output_mint) |
| 6 | Input Mint | no | no | input_mint |
| 7 | Output Mint | no | no | output_mint |
| 8 | Token Program | no | no | SPL Token (`TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`) |
| 9 | Event Authority | no | no | PDA: seeds=["__event_authority"], program=DAMM_v2 |
| 10 | Program | no | no | DAMM v2 program ID |
| 11 | Payer | yes | yes | searcher_keypair |

**Additional:** Transaction must include Instruction Sysvar account (`Sysvar1nstructions1111111111111111111111111`).

**Direction logic:** If `input_mint == token_a_mint` (offset 168), then input vault = token_a_vault (232), output vault = token_b_vault (264). Otherwise reversed.

## Pool State Extension

The current `PoolState` struct stores mints and reserves but not vault pubkeys or config accounts. For building swap instructions we need:
- Vault pubkeys (already parsed but not stored — Category B parsers return them separately)
- AMM config pubkey (Raydium CP offset 8)
- Token program pubkeys (Raydium CP offsets 232, 264)

**Approach:** Add an `extra: Option<PoolExtra>` field to PoolState:

```rust
pub struct PoolExtra {
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub config: Option<Pubkey>,       // Raydium CP amm_config
    pub token_program_a: Option<Pubkey>, // Token-2022 support
    pub token_program_b: Option<Pubkey>,
}
```

All parsers already extract vaults — just store them. CP parser also extracts amm_config and token programs.

## Deduplication

In the router loop, track recently processed pools:

```rust
let mut recent_pools: HashMap<Pubkey, u64> = HashMap::new(); // pool -> last_slot
// Before processing:
if recent_pools.get(&pool_address) == Some(&change.slot) { continue; }
recent_pools.insert(pool_address, change.slot);
// Evict old entries every 100 slots
```

## Route Filtering

Only submit routes where ALL hops have real swap IX builders. Add a method:

```rust
fn can_build_real_ix(dex_type: DexType) -> bool {
    matches!(dex_type, DexType::RaydiumCp | DexType::MeteoraDammV2)
}
```

Before submitting a bundle, check all hops. If any hop can't build a real IX, log it as "dry-run only" and skip submission.

## Bundle Builder Changes

`BundleBuilder` needs cache access to look up pool state (for vault pubkeys, config). Pass `StateCache` to the builder or to individual build methods.

## Files Changed

| File | Action | What |
|------|--------|------|
| `src/router/pool.rs` | Modify | Add `PoolExtra` struct and `extra: Option<PoolExtra>` field to PoolState |
| `src/mempool/stream.rs` | Modify | Store vault pubkeys + config in PoolExtra for all parsers |
| `src/executor/bundle.rs` | Modify | Real swap IX builders for CP and DAMM v2, cache access |
| `src/main.rs` | Modify | Add dedup, route filtering, pass cache to bundle builder |
| `tests/unit/bundle_cp.rs` | Create | Unit tests for CP swap IX |
| `tests/unit/bundle_damm.rs` | Create | Unit tests for DAMM v2 swap IX |
| `tests/unit/mod.rs` | Modify | Add new test modules |

## Testing

- Unit tests: construct known pool state, build swap IX, verify account count, order, and signer flags
- Manual: run with DRY_RUN=false on a funded keypair, verify bundles land on Jito
