# Orderbook DEX Integration: Phoenix + Manifest

**Date:** 2026-04-02
**Status:** Draft
**Scope:** Add Phoenix V1 and Manifest CLOB DEXes to the Geyser streaming pipeline, route calculator, profit simulator, and bundle builder.

## Motivation

The engine currently supports 6 AMM DEXes. All arb routes are AMM-to-AMM. On Solana, the dominant MEV pattern is **AMM-to-CLOB arbitrage**: stale AMM prices are offset against orderbook venues where market makers have already updated quotes. Adding Phoenix and Manifest unlocks this pattern.

**Why these two:**
- Phoenix: $75B+ total volume, dominant CLOB on Solana, open-source (Ellipsis Labs)
- Manifest: Zero-fee CLOB, growing rapidly, open-source (CKS Systems / Mango founder), O(1) top-of-book access
- Both are integrated into Jupiter Metis routing

**What we investigated and rejected:**
- Prop AMMs (HumidiFi, SolFi, Obric, BisonFi, GoonFi): Closed-source, anti-MEV by design, opaque state
- Jupiter RFQ V2: Off-chain quotes, incompatible with Geyser-based detection
- PancakeSwap: Low Solana volume, low priority

## Architecture: How Orderbooks Differ from AMMs

Current AMM pools have **fixed-size accounts** with reserves at known offsets. We route by `data.len()` in `stream.rs`. Orderbooks are different:

| Property | AMMs (current) | Orderbooks (new) |
|----------|---------------|-----------------|
| Account size | Fixed per DEX | Variable (grows with orders) |
| Pricing | Derived from reserves (CPMM/CLMM math) | Read from order tree (best bid/ask) |
| Data structure | Flat fields at byte offsets | Header + Red-Black tree |
| Fee model | Fee applied to input amount | Phoenix: taker fees; Manifest: zero fees |
| Identification | Route by `data.len()` | Route by 8-byte discriminant at offset 0 |
| Output calculation | Formula (reserves, liquidity) | Walk the book (consume orders until input exhausted) |

**Key design decision:** We cannot use `data.len()` to route orderbook accounts because their size varies. Instead, we match on the **8-byte discriminant** at offset 0 of the account data, falling through to discriminant-based matching for accounts that don't match any known AMM size.

## Phoenix V1 Integration

**Program ID:** `PhoeNiXZ8ByJGLkxNfZRnkUfjvmuYqLR89jjFHGqdXY`

### Account Layout

Phoenix market accounts have a fixed **MarketHeader (624 bytes)** followed by a variable-size FIFOMarket containing Red-Black trees for bids, asks, and trader seats.

**MarketHeader key fields:**

| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | discriminant (u64) |
| 8 | 8 | status (u64) |
| 16 | 24 | market_size_params: {bids_size, asks_size, num_seats} (3x u64) |
| 40 | 96 | base_params: TokenParams {decimals(u32), vault_bump(u32), mint_key(Pubkey), vault_key(Pubkey)} |
| 48 | 32 | — base_mint (at base_params + 8) |
| 80 | 32 | — base_vault (at base_params + 40) |
| 136 | 8 | base_lot_size (u64) |
| 144 | 96 | quote_params: TokenParams (same layout) |
| 152 | 32 | — quote_mint (at quote_params + 8) |
| 184 | 32 | — quote_vault (at quote_params + 40) |
| 240 | 8 | quote_lot_size (u64) |
| 248 | 8 | tick_size_in_quote_atoms_per_base_unit (u64) |

### Top-of-Book Extraction

Phoenix stores the orderbook as a **sokoban RedBlackTree**. There is no fixed "best bid" field. To get top-of-book:

1. Parse the MarketHeader (624 bytes) to get `market_size_params`
2. Use `phoenix-sdk-core` crate's `load_with_dispatch()` to deserialize the correct FIFOMarket variant
3. Call `get_book(Side::Bid).iter().next()` for best bid, same for asks

**Dependency:** `phoenix-sdk-core` crate (from Ellipsis-Labs). We use this rather than writing a custom parser because the sokoban tree layout is complex and version-sensitive.

### Output Calculation

To estimate swap output (for route simulation):

```
Walk the book from best price, consuming orders:
  remaining_input = input_amount
  total_output = 0
  for each order at price P (in quote_atoms_per_base_unit):
    fillable = min(remaining_input, order.size_in_base_lots * base_lot_size)
    output = fillable * P * quote_lot_size / base_lot_size  (simplified)
    total_output += output
    remaining_input -= fillable
    if remaining_input == 0: break
  apply taker fee (deduct from total_output)
```

Phoenix taker fee is per-market. Read from the market header or default to ~2-4 bps for major markets.

### Swap Instruction

**Discriminant:** `0x00` (Swap)

**Accounts (9):**
1. Phoenix program (readonly)
2. Log authority PDA (readonly)
3. Market (writable)
4. Trader/signer (signer, writable)
5. Trader base token account (writable)
6. Trader quote token account (writable)
7. Base vault PDA (writable)
8. Quote vault PDA (writable)
9. Token Program (readonly)

**Instruction data:** 1 byte discriminant + serialized `OrderPacket::ImmediateOrCancel { ... }`

We use the `phoenix-sdk-core` types to construct the OrderPacket.

## Manifest Integration

**Program ID:** `MNFSTqtC93rEfYHB6hF82sKdZpUDFWkViLByLd1k1Ms`

### Account Layout

Manifest market accounts have a fixed **MarketFixed header (256 bytes)** followed by variable-size Red-Black tree nodes (80 bytes each).

**MarketFixed key fields:**

| Offset | Size | Field |
|--------|------|-------|
| 0 | 8 | discriminant (u64) |
| 8 | 1 | version |
| 9 | 1 | base_mint_decimals |
| 10 | 1 | quote_mint_decimals |
| 16 | 32 | base_mint (Pubkey) |
| 48 | 32 | quote_mint (Pubkey) |
| 80 | 32 | base_vault (Pubkey) |
| 112 | 32 | quote_vault (Pubkey) |
| 156 | 4 | bids_root_index (u32) |
| 160 | 4 | **bids_best_index** (u32) |
| 164 | 4 | asks_root_index (u32) |
| 168 | 4 | **asks_best_index** (u32) |

### Top-of-Book Extraction — O(1)

Unlike Phoenix, Manifest stores `bids_best_index` and `asks_best_index` directly in the header. This gives **O(1) top-of-book access**:

1. Read `bids_best_index` (u32 at offset 160)
2. If non-zero: best bid price is at `data[256 + best_index * 16 .. +16]` (the RestingOrder's price field, a QuoteAtomsPerBaseAtom u128)
3. Same for `asks_best_index` at offset 168

**Note:** The index math depends on the tree node alignment. We should use the `manifest-dex` crate for correct deserialization rather than hardcoding offsets into the dynamic portion.

**Dependency:** `manifest-dex` crate (from CKS-Systems).

### Output Calculation

Manifest has **zero trading fees**. The output calculation walks the book:

```
remaining_input = input_amount
total_output = 0
for each order at price P:
  fillable = min(remaining_input, order.num_base_atoms)
  output = fillable * price  (price is QuoteAtomsPerBaseAtom, fixed-point)
  total_output += output
  remaining_input -= fillable
  if remaining_input == 0: break
// No fee deduction
```

### Swap Instruction

**Discriminant:** single byte `4` (Swap) or `13` (SwapV2)

**Swap accounts:**
0. Payer (signer, writable)
1. Market (writable)
2. System program
3. Trader base token account (writable)
4. Trader quote token account (writable)
5. Base vault PDA (writable) — seeds: `[b"vault", market, base_mint]`
6. Quote vault PDA (writable) — seeds: `[b"vault", market, quote_mint]`
7. Token program base
8+. Optional: base_mint, token_program_quote, quote_mint (for Token-2022)

**Instruction data:** 1 byte discriminant + `SwapParams { in_atoms: u64, out_atoms: u64, is_base_in: bool, is_exact_in: bool }`

## Changes to Existing Architecture

### 1. DexType Enum (`router/pool.rs`)

Add two new variants:
```rust
pub enum DexType {
    // ... existing ...
    Phoenix,
    Manifest,
}
```

### 2. PoolState Adaptation for Orderbooks

Current `PoolState` uses `token_a_reserve`/`token_b_reserve` and `get_output_amount()` with CPMM/CLMM math. Orderbooks don't have reserves — they have an order book.

**Approach:** Store synthetic top-of-book data in PoolState for route discovery:
- `token_a_reserve` = best bid size (total depth at top N levels in base atoms)
- `token_b_reserve` = best ask size (total depth at top N levels in quote atoms)
- `fee_bps` = taker fee (Phoenix) or 0 (Manifest)
- New field: `best_bid_price: Option<u128>` — price in quote atoms per base atom
- New field: `best_ask_price: Option<u128>` — price in quote atoms per base atom

For output calculation, `get_output_amount()` gets a new branch:
- If `best_bid_price`/`best_ask_price` is set, use orderbook math (price * amount) instead of CPMM/CLMM
- This is approximate (uses only top-of-book, not full depth) but conservative — underestimates output for large trades, which is safe

For precise simulation in the profit simulator, we re-read the full account data and walk the book.

### 3. Geyser Subscription (`config.rs` + `stream.rs`)

Add Phoenix and Manifest program IDs to `monitored_programs()`.

In `process_update()`, change the routing logic:
```rust
let parsed = match data.len() {
    653 => parse_orca_whirlpool(...),
    1560 => parse_raydium_clmm(...),
    // ... existing fixed-size matches ...
    _ => {
        // Variable-size accounts: check discriminant
        if data.len() >= 624 {
            try_parse_phoenix(...)
        } else if data.len() >= 256 {
            try_parse_manifest(...)
        } else {
            None
        }
    }
};
```

Phoenix and Manifest parsers check the 8-byte discriminant at offset 0 to confirm the account type before parsing.

### 4. Bundle Builder (`executor/bundle.rs`)

Add `DexType::Phoenix` and `DexType::Manifest` branches in `build_swap_instruction_with_min_out()`.

### 5. New Dependencies

```toml
# Cargo.toml
phoenix-sdk-core = "..."   # Phoenix orderbook deserialization
manifest-dex = "..."        # Manifest market deserialization
```

We use the official SDK crates rather than writing custom parsers. This is a deliberate trade-off: adds dependencies but avoids fragile byte-offset parsing of complex tree structures.

## Route Discovery Impact

Adding orderbooks to the route calculator enables new route types:

- **AMM → CLOB:** Detect stale AMM price, offset on orderbook (the dominant MEV pattern)
- **CLOB → AMM:** Detect stale orderbook quote, offset on AMM
- **AMM → CLOB → AMM:** 3-hop routes through orderbooks

The existing `calculator.rs` token-to-pool index handles this automatically — Phoenix/Manifest pools are indexed by their token pairs like any other pool.

## Testing Strategy

1. **Unit tests:** Parse known Phoenix and Manifest account data (captured from mainnet via `getAccountInfo`) and verify extracted pricing matches expected values
2. **Output calculation tests:** Verify orderbook walk math against known trades
3. **Swap IX tests:** Verify instruction serialization matches expected format
4. **Integration test:** Subscribe to Phoenix/Manifest via Geyser on mainnet, verify we receive and parse market updates

## Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| Phoenix SDK crate version breaks | Pin version, test in CI |
| Large account data (multi-MB Phoenix markets) | Only parse header + top-of-book for route discovery; full parse only in simulator |
| Orderbook updates are very frequent (every slot) | Same as AMMs — Geyser handles this; we only process if price moved |
| Phoenix has 12 market configs (different sizes) | SDK handles dispatch via `load_with_dispatch()` |
| Manifest tree node alignment changes | Use `manifest-dex` crate, don't hardcode offsets into dynamic data |

## Out of Scope

- Full orderbook depth storage (we only need top-of-book for route discovery)
- Limit order placement (we only do IOC swaps)
- Market making on orderbooks (different strategy entirely)
- OpenBook V2 (lower priority, can add later following same pattern)
