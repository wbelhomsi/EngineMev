# PumpSwap AMM Integration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add PumpSwap AMM (Pump.fun graduated tokens) as the 10th DEX, enabling arb routes through the highest-dislocation memecoin pools.

**Architecture:** Geyser subscription by program owner → pool state parser → lazy vault fetch → CPMM pricing with 125 bps conservative fee → swap IX builder (sell: 21 accounts, buy: 23 accounts) → execute_arb_v2 CPI. Follows exact same pattern as existing DEXes.

**Tech Stack:** Rust, solana-sdk 4.0, PumpSwap program `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA`

---

## File Structure

| File | Responsibility | Change |
|------|---------------|--------|
| `src/addresses.rs` | MODIFY | Add PumpSwap const addresses (program, global config, event auth, fee config, fee program, volume accumulator) |
| `src/router/pool.rs` | MODIFY | Add `DexType::PumpSwap`, add `coin_creator`/`is_mayhem_mode`/`is_cashback_coin` to PoolExtra, add 125 bps fee |
| `src/router/mod.rs` | MODIFY | Add PumpSwap to `can_submit_route()` |
| `src/config.rs` | MODIFY | Add PumpSwap to `monitored_programs()` |
| `src/mempool/stream.rs` | MODIFY | Add `parse_pumpswap()`, discriminator routing, vault fetch |
| `src/executor/bundle.rs` | MODIFY | Add `build_pumpswap_swap_ix()` |
| `src/bin/setup_alt.rs` | MODIFY | Add ~23 PumpSwap addresses to ALT |
| `tests/unit/stream_parsing.rs` | MODIFY | PumpSwap parser tests |
| `tests/unit/bundle_real_ix.rs` | MODIFY | PumpSwap swap IX tests |
| `tests/unit/submission_filter.rs` | MODIFY | PumpSwap route accepted |

---

### Task 1: Add DexType::PumpSwap + PoolExtra fields + addresses

**Files:**
- Modify: `src/addresses.rs`
- Modify: `src/router/pool.rs`
- Modify: `src/router/mod.rs`
- Modify: `src/config.rs`

- [ ] **Step 1: Add PumpSwap addresses to addresses.rs**

Add after the existing `MANIFEST` constant in `src/addresses.rs`. Convert each base58 address to `Pubkey::new_from_array([...])` using the same pattern as existing addresses.

Addresses to add:
- `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA` — PUMPSWAP
- `ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw` — PUMPSWAP_GLOBAL_CONFIG
- `GS4CU59F31iL7aR2Q8zVS8DRrcRnXX1yjQ66TqNVQnaR` — PUMPSWAP_EVENT_AUTHORITY
- `5PHirr8joyTMp9JMm6nW7hNDVyEYdkzDqazxPD7RaTjx` — PUMPSWAP_FEE_CONFIG
- `pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ` — PUMPSWAP_FEE_PROGRAM
- `C2aFPdENg4A2HQsmrd5rTw5TaYBX5Ku887cWjbFKtZpw` — PUMPSWAP_GLOBAL_VOLUME_ACCUMULATOR

Use a Rust script or `solana-keygen pubkey` to get the byte arrays. Or use `Pubkey::from_str` in a `LazyLock` like the config.rs pattern. Actually, follow the existing addresses.rs pattern: `pub const NAME: Pubkey = Pubkey::new_from_array([...]);`

- [ ] **Step 2: Add DexType::PumpSwap to pool.rs**

In `src/router/pool.rs`, add `PumpSwap` to the `DexType` enum. Add the fee in `base_fee_bps()`:

```rust
DexType::PumpSwap => 125, // conservative worst-case (tiered 30-125 bps)
```

Add fields to `PoolExtra`:

```rust
pub coin_creator: Option<Pubkey>,
pub is_mayhem_mode: Option<bool>,
pub is_cashback_coin: Option<bool>,
```

- [ ] **Step 3: Add PumpSwap to can_submit_route in router/mod.rs**

Read `src/router/mod.rs`, find `can_submit_route`. Add `DexType::PumpSwap` to the match arm that returns `true`.

- [ ] **Step 4: Add PumpSwap to monitored_programs in config.rs**

In `src/config.rs`, find `monitored_programs()`. Add `addresses::PUMPSWAP` to the vec.

- [ ] **Step 5: Verify compilation**

Run: `cargo check`
Expected: compiles (PumpSwap not used yet, but types exist)

- [ ] **Step 6: Commit**

```bash
git add src/addresses.rs src/router/pool.rs src/router/mod.rs src/config.rs
git commit -m "feat: add DexType::PumpSwap + addresses + PoolExtra fields

PumpSwap program ID, global config, event authority, fee config,
fee program, volume accumulator. 125 bps conservative fee.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: Pool parser + Geyser routing (TDD)

**Files:**
- Modify: `src/mempool/stream.rs`
- Modify: `tests/unit/stream_parsing.rs`

- [ ] **Step 1: Write failing parser tests**

Add to `tests/unit/stream_parsing.rs`:

```rust
#[test]
fn test_parse_pumpswap_pool() {
    use solana_mev_bot::mempool::stream::parse_pumpswap;
    use solana_sdk::pubkey::Pubkey;

    // Build a 245-byte PumpSwap pool account
    let mut data = vec![0u8; 245];
    // Discriminator at offset 0
    data[0..8].copy_from_slice(&[0xf1, 0x9a, 0x6d, 0x04, 0x11, 0xb1, 0x6d, 0xbc]);
    // pool_bump at offset 8
    data[8] = 254;
    // index at offset 9 (u16 LE)
    data[9..11].copy_from_slice(&0u16.to_le_bytes());
    // creator at offset 11
    let creator = Pubkey::new_unique();
    data[11..43].copy_from_slice(creator.as_ref());
    // base_mint at offset 43
    let base_mint = Pubkey::new_unique();
    data[43..75].copy_from_slice(base_mint.as_ref());
    // quote_mint at offset 75 (wSOL)
    let quote_mint = solana_mev_bot::config::sol_mint();
    data[75..107].copy_from_slice(quote_mint.as_ref());
    // lp_mint at offset 107
    data[107..139].copy_from_slice(Pubkey::new_unique().as_ref());
    // base vault at offset 139
    let base_vault = Pubkey::new_unique();
    data[139..171].copy_from_slice(base_vault.as_ref());
    // quote vault at offset 171
    let quote_vault = Pubkey::new_unique();
    data[171..203].copy_from_slice(quote_vault.as_ref());
    // lp_supply at offset 203
    data[203..211].copy_from_slice(&1000000u64.to_le_bytes());
    // coin_creator at offset 211
    let coin_creator = Pubkey::new_unique();
    data[211..243].copy_from_slice(coin_creator.as_ref());
    // is_mayhem_mode at offset 243
    data[243] = 0;
    // is_cashback_coin at offset 244
    data[244] = 1;

    let pool_address = Pubkey::new_unique();
    let result = parse_pumpswap(&pool_address, &data, 100);
    assert!(result.is_some(), "Should parse valid PumpSwap pool");

    let (pool, (v_a, v_b)) = result.unwrap();
    assert_eq!(pool.dex_type, solana_mev_bot::router::pool::DexType::PumpSwap);
    assert_eq!(pool.token_a_mint, base_mint);
    assert_eq!(pool.token_b_mint, quote_mint);
    assert_eq!(pool.fee_bps, 125);
    assert_eq!(pool.token_a_reserve, 0); // not yet fetched
    assert_eq!(pool.token_b_reserve, 0);
    assert_eq!(v_a, base_vault);
    assert_eq!(v_b, quote_vault);
    assert_eq!(pool.extra.coin_creator, Some(coin_creator));
    assert_eq!(pool.extra.is_cashback_coin, Some(true));
}

#[test]
fn test_parse_pumpswap_wrong_discriminator() {
    let mut data = vec![0u8; 245];
    data[0..8].copy_from_slice(&[0x00; 8]); // wrong discriminator
    let result = solana_mev_bot::mempool::stream::parse_pumpswap(
        &solana_sdk::pubkey::Pubkey::new_unique(), &data, 0);
    assert!(result.is_none());
}

#[test]
fn test_parse_pumpswap_too_short() {
    let data = vec![0u8; 200]; // too short (min 243)
    let result = solana_mev_bot::mempool::stream::parse_pumpswap(
        &solana_sdk::pubkey::Pubkey::new_unique(), &data, 0);
    assert!(result.is_none());
}

#[test]
fn test_parse_pumpswap_243_bytes_no_optional() {
    let mut data = vec![0u8; 243];
    data[0..8].copy_from_slice(&[0xf1, 0x9a, 0x6d, 0x04, 0x11, 0xb1, 0x6d, 0xbc]);
    data[43..75].copy_from_slice(solana_sdk::pubkey::Pubkey::new_unique().as_ref()); // base_mint
    data[75..107].copy_from_slice(solana_mev_bot::config::sol_mint().as_ref()); // quote_mint
    data[139..171].copy_from_slice(solana_sdk::pubkey::Pubkey::new_unique().as_ref()); // base vault
    data[171..203].copy_from_slice(solana_sdk::pubkey::Pubkey::new_unique().as_ref()); // quote vault
    data[211..243].copy_from_slice(solana_sdk::pubkey::Pubkey::new_unique().as_ref()); // coin_creator

    let result = solana_mev_bot::mempool::stream::parse_pumpswap(
        &solana_sdk::pubkey::Pubkey::new_unique(), &data, 0);
    assert!(result.is_some());
    let (pool, _) = result.unwrap();
    assert_eq!(pool.extra.is_mayhem_mode, Some(false)); // default
    assert_eq!(pool.extra.is_cashback_coin, Some(false)); // default
}
```

- [ ] **Step 2: Run tests — should FAIL** (parse_pumpswap doesn't exist)

Run: `cargo test --test unit stream_parsing`

- [ ] **Step 3: Implement parse_pumpswap in stream.rs**

Add `pub fn parse_pumpswap(pool_address: &Pubkey, data: &[u8], slot: u64) -> Option<(PoolState, (Pubkey, Pubkey))>` following the byte offsets from the spec. Return `None` if data < 243 bytes or discriminator doesn't match.

Also add routing in the Geyser parsing dispatch — add a branch that checks the PumpSwap discriminator. Since PumpSwap sizes (243-301) don't overlap with existing DEX sizes (637+), add it as a catch-all or add specific size matches.

**Important:** Follow the Raydium CP pattern for vault fetch — spawn async `fetch_vault_balances_for_pool` BEFORE sending `PoolStateChange` to the channel. This prevents the false positive issue.

- [ ] **Step 4: Run tests — should PASS**

Run: `cargo test --test unit stream_parsing`

- [ ] **Step 5: Commit**

```bash
git add src/mempool/stream.rs tests/unit/stream_parsing.rs
git commit -m "feat: PumpSwap pool parser + Geyser routing

Parses pool state at known offsets, extracts mints/vaults/creator.
Routes by discriminator [0xf1,0x9a,...]. Lazy vault fetch for reserves.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: Swap IX builder (TDD)

**Files:**
- Modify: `src/executor/bundle.rs`
- Modify: `tests/unit/bundle_real_ix.rs`

- [ ] **Step 1: Write failing swap IX tests**

Add to `tests/unit/bundle_real_ix.rs`:

```rust
#[test]
fn test_pumpswap_sell_ix_account_count() {
    // Build a PoolState with PumpSwap fields
    // Call build_pumpswap_swap_ix for sell direction
    // Assert: 21 accounts
}

#[test]
fn test_pumpswap_sell_ix_discriminator() {
    // Assert: data starts with [51, 230, 133, 164, 1, 127, 131, 173]
}

#[test]
fn test_pumpswap_buy_ix_account_count() {
    // Assert: 23 accounts (21 + volume accumulator + global volume)
}

#[test]
fn test_pumpswap_sell_ix_returns_none_without_vaults() {
    // PoolExtra with vault_a=None → returns None
}

#[test]
fn test_pumpswap_sell_ix_returns_none_without_coin_creator() {
    // PoolExtra with coin_creator=None → returns None
}
```

Write full test bodies following the existing pattern in `bundle_real_ix.rs`. Each test creates a PoolState with `dex_type: DexType::PumpSwap`, populates the required `PoolExtra` fields, and calls `build_pumpswap_swap_ix`.

- [ ] **Step 2: Run tests — should FAIL**

- [ ] **Step 3: Implement build_pumpswap_swap_ix in bundle.rs**

Add the function following the 21-account sell / 23-account buy layout from the spec. Key implementation details:

- `coin_creator_vault_authority`: PDA `["creator_vault", coin_creator]` on PumpSwap program
- `coin_creator_vault_ata`: standard ATA from (authority, quote_mint)
- `protocol_fee_recipient`: round-robin from 8 hardcoded addresses (use AtomicUsize counter)
- `user_volume_accumulator`: PDA `["user_volume_accumulator", signer]` on PumpSwap program
- For sell: no volume accumulator accounts needed (unless cashback)
- For buy: always include global_volume_accumulator + user_volume_accumulator

Also add `DexType::PumpSwap` to `build_swap_instruction_with_min_out` match arm.

- [ ] **Step 4: Run tests — should PASS**

- [ ] **Step 5: Commit**

```bash
git add src/executor/bundle.rs tests/unit/bundle_real_ix.rs
git commit -m "feat: PumpSwap swap IX builder (sell 21 accts, buy 23 accts)

Sell discriminator [51,230,...], Buy discriminator [102,6,...].
Coin creator vault PDA, protocol fee round-robin, volume accumulator.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: Submission filter + pricing test

**Files:**
- Modify: `tests/unit/submission_filter.rs`

- [ ] **Step 1: Add PumpSwap to submission filter test**

```rust
#[test]
fn test_pumpswap_route_accepted() {
    // Create a route with DexType::PumpSwap
    // Assert can_submit_route returns true
}
```

- [ ] **Step 2: Run test — should PASS** (already added PumpSwap to can_submit_route in Task 1)

- [ ] **Step 3: Add CPMM pricing test with 125 bps fee**

```rust
#[test]
fn test_pumpswap_cpmm_output_with_125bps_fee() {
    // Create PumpSwap pool with known reserves
    // Call get_output_amount
    // Verify output matches CPMM formula with 125 bps fee deduction
    // Compare: 125 bps fee gives LESS output than 25 bps fee
}
```

- [ ] **Step 4: Commit**

```bash
git add tests/unit/submission_filter.rs
git commit -m "test: PumpSwap submission filter + CPMM pricing verification

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: ALT expansion + docs

**Files:**
- Modify: `src/bin/setup_alt.rs`
- Modify: `CLAUDE.md`
- Modify: `.env.example`

- [ ] **Step 1: Add PumpSwap addresses to setup_alt.rs**

Add ~23 addresses:
- PumpSwap program
- Global Config
- Event Authority
- Fee Config
- Fee Program
- Global Volume Accumulator
- 8 protocol fee recipients
- 8 protocol fee recipient wSOL ATAs
- User volume accumulator PDA (derived from signer)

- [ ] **Step 2: Run setup_alt to extend the on-chain ALT**

```bash
cargo run --release --bin setup_alt
```

This will detect the new addresses and extend the ALT on-chain.

- [ ] **Step 3: Update CLAUDE.md**

- Add PumpSwap to the DEX Program IDs table
- Add `DexType::PumpSwap` to the module map
- Update the monitored programs count (9 → 10)
- Add PumpSwap data size info
- Update test count

- [ ] **Step 4: Commit and push**

```bash
git add src/bin/setup_alt.rs CLAUDE.md
git commit -m "feat: expand ALT with PumpSwap addresses + update docs

23 new ALT addresses: program, global config, event authority,
fee config, fee program, volume accumulator, 8 fee recipients + ATAs.

Co-Authored-By: Claude <noreply@anthropic.com>"
git push
```

---

### Task 6: Final integration test + validation

**Files:** All

- [ ] **Step 1: Full test suite**

```bash
cargo test --test unit
cargo test --features e2e --test e2e
cargo clippy --all-targets -- -D warnings
cargo check --features e2e_surfpool --tests
```

Expected: all pass, 0 warnings

- [ ] **Step 2: Build release**

```bash
cargo build --release
```

- [ ] **Step 3: Quick dry-run validation (30 seconds)**

```bash
DRY_RUN=true RUST_LOG=solana_mev_bot=debug timeout 30 cargo run --release --bin solana-mev-bot 2>&1 | grep -i "pumpswap\|pump"
```

Verify: PumpSwap pools are being discovered via Geyser, parsed, and vault fetches are happening.

- [ ] **Step 4: Commit any fixups**

```bash
git add -A
git commit -m "fix: integration fixes for PumpSwap

Co-Authored-By: Claude <noreply@anthropic.com>"
git push
```
