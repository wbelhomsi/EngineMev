# Manifest CLOB Market Making — Design Spec

**Date:** 2026-04-18
**Status:** v1 dry-run scaffold committed; live-submission path pending.

## Goal

Build a halal-compliant passive market-making bot for Manifest CLOB that
continuously posts resting bid/ask around a CEX reference mid and earns
the spread from counterparty flow.

This is the retail-accessible alternative to the cexdex arb binary,
which validated (after two extensive runs) that generic spot arbitrage
at $1-5k capital cannot compete with co-located professional bots.

## Why Manifest

After research (see `docs/superpowers/research/2026-04-18-solana-mev-landscape.md`
for the consolidated agent reports):

- **Permissionless** — "professional market makers are not required to do
  anything to gain quoting privileges" (from Manifest docs). Unlike JupiterZ
  / Hashflow / 1inch Solana Fusion, which are de-facto institutional.
- **Zero taker fees** — makers earn the full spread.
- **Our parser already exists** (`src/mempool/parsers/manifest.rs`).
- **Competition is other market makers**, not co-located sub-ms arb bots.
  Edge becomes *quoting quality*, not microseconds.

## Scope

**In scope for v1 (this commit):**

1. Manifest instruction builders (ClaimSeat / Deposit / Withdraw / BatchUpdate)
2. Pure-function quoter: CEX mid + inventory ratio → (bid, ask)
3. Local book-state tracking of our resting orders
4. Env-driven config
5. Dry-run binary that exercises the full quoting loop against live
   Binance WS and logs would-be IX payloads to JSONL — no on-chain submission

**Follow-up commits (not in v1):**

- Real bundle submission (blockhash + signing + relay dispatch)
- Fill detection via periodic Geyser poll of the market account
- Inventory state sync (seat balance on-chain + wallet balances)
- Binance hedge execution when inventory drifts
- Prometheus metrics (quote rate, fill rate, inventory delta, P&L)
- Multi-market support (currently single market per process)

## Architecture

Mirrors `src/cexdex/` structure, inverted from taker to maker:

```
Binance WS ──► PriceStore ──► Quoter ──► (bid_price, ask_price)
                                          │
BookState ────────────────────────────────┤
(our live orders)                          ▼
                               build_batch_update_ix
                                          │
                                          ▼
                               [dry-run: log] / [live: sign + submit]
                                          │
                   Fill Detector (Geyser poll) ──► BookState update
                                                   Inventory update
                                                   Hedger (follow-up)
```

## Module Structure

```
src/mm/
├── mod.rs            # re-exports
├── config.rs         # MmConfig from MM_* env vars
├── quoter.rs         # pure (cex_mid, inventory_ratio) → (bid, ask)
└── book_state.rs     # track our live orders by seq_number

src/executor/swaps/
└── manifest_mm.rs    # ClaimSeat / Deposit / Withdraw / BatchUpdate IX builders

src/bin/
└── manifest_mm.rs    # orchestrator binary
```

## Quoter Logic

Given CEX mid `M` and inventory ratio `R` (base share of portfolio, 0..1):

1. Compute skew:
   - `δ = R - target_ratio` (e.g. target = 0.5)
   - Normalize to [-1, +1] by `window`: `norm = clamp(-δ / window, -1, 1)`
   - `skew = norm * max_skew_frac`
2. Adjusted mid: `M' = M * (1 + skew)`
   - When we're base-heavy → `skew < 0` → shift mid down → ask gets more aggressive
   - When we're quote-heavy → `skew > 0` → shift mid up → bid gets more aggressive
3. Symmetric spread around `M'`:
   - `bid = M' * (1 - half_spread)`
   - `ask = M' * (1 + half_spread)`
4. Apply `min_half_spread_frac` as hard floor.

## Price Encoding

Manifest stores prices as `u32 mantissa × 10^(i8 exponent)` (quote atoms per base atom).

Our `encode_price_mantissa` picks an exponent that scales the mantissa to
~1e8 (9 sig figs) and clamps to u32.

## Halal Posture

- Market making on spot pairs = textbook-permissible. No riba, no gharar
  (outcomes deterministic: post order, gets filled or cancelled), no maysir.
- Only trade pairs where BOTH base and quote are halal instruments. For SOL
  markets this is fine; for LST pairs (jitoSOL/USDC, etc.) also fine per
  Sanctum's posture.
- Deliberately NOT supporting: Phoenix Perps (leverage), Drift Perps, JLP
  (leveraged instruments in the basket), lending-adjacent protocols.

## Safety Gates (built into v1)

- **Dry-run default** — MM_DRY_RUN=true is the default; must be explicitly
  disabled to submit on-chain.
- **Stale CEX** — if Binance snapshot is older than `MM_CEX_STALENESS_MS`,
  the quote cycle is skipped entirely (no flying blind).
- **Min half-spread** — `MM_MIN_HALF_SPREAD_FRAC` prevents quoting tighter
  than a configured floor even if skew would push us there.
- **PostOnly order type** — orders will be rejected rather than cross the
  book, preventing accidental taker trades at bad prices.

## Configuration (env vars)

| Var | Default | Purpose |
|---|---|---|
| `MM_SEARCHER_PRIVATE_KEY` | required | base58 signer |
| `MM_MARKET` | required | Manifest market pubkey |
| `MM_BASE_MINT` | required | base token mint |
| `MM_QUOTE_MINT` | USDC | quote token mint |
| `MM_BASE_DECIMALS` | 9 | base token decimals |
| `MM_QUOTE_DECIMALS` | 6 | quote token decimals |
| `MM_CEX_REFERENCE_SYMBOL` | SOLUSDC | Binance symbol |
| `MM_HALF_SPREAD_FRAC` | 0.0005 | 5 bps each side |
| `MM_MAX_SKEW_FRAC` | 0.001 | max inventory-driven skew |
| `MM_TARGET_INVENTORY_RATIO` | 0.5 | balanced book |
| `MM_SKEW_RATIO_WINDOW` | 0.3 | full-skew threshold |
| `MM_MIN_HALF_SPREAD_FRAC` | 0.0002 | floor |
| `MM_ORDER_SIZE_BASE_ATOMS` | 100_000_000 | 0.1 SOL if base=SOL |
| `MM_REQUOTE_INTERVAL_MS` | 500 | quote cycle period |
| `MM_CEX_STALENESS_MS` | 500 | max snapshot age |
| `MM_REQUOTE_THRESHOLD_FRAC` | 0.0002 | mid-move requote trigger |
| `MM_DRY_RUN` | true | log-only mode |
| `MM_RUN_SECS` | 0 | auto-shutdown (0 = forever) |
| `MM_STATS_PATH` | /tmp/manifest_mm | log prefix |
| `MM_METRICS_PORT` | 0 | Prometheus (0 = disabled) |

## Validation Plan Before Live Capital

1. **Dry-run against real Binance feed for 24h** — confirm quoter produces
   sane bid/ask against real-world SOL/USDC price movement. ✅ achievable now.
2. **Implement live submission path** — blockhash, sign, submit via Jito.
3. **Dry-submit to Manifest devnet** — validate IX shape lands.
4. **Live deploy with $200 in seat** on a thin-volume halal market (LST/USDC
   probably), measure fill rate + spread capture for 1 week.
5. **Scale to $1k+ only if** net P&L is positive after fees AND no inventory
   blow-ups.

## Open Questions (address before live)

- **Which market to target first?** Needs on-chain discovery of Manifest
  markets with halal pairs. Candidates: jitoSOL/USDC, JupSOL/USDC,
  mSOL/USDC if they exist. Needs a "find markets" helper.
- **Fill detection cadence** — polling interval that balances staleness
  vs RPC budget. Probably 1-2s given Manifest's market-account update rate.
- **How to handle partial fills** — Manifest supports partial fills;
  BookState needs to handle updates to `base_atoms` on an existing order.
- **Seat TVL minimum** — Manifest may have dust-prevention floors; test on
  devnet.

## Test Coverage (this commit)

- `manifest_mm::tests` — 6 IX builder tests (shape, borsh round-trip, PDA)
- `mm::quoter::tests` — 6 quoter tests (symmetric, skew direction, clamp,
  min spread, ordering invariants)
- `mm::book_state::tests` — 4 tests (insert/iterate, remove, seq enumeration,
  price reconstruction)
- `mm::config::tests` — 1 test (env float default)
- `bin::manifest_mm::tests` — 2 tests (price-mantissa round trip, bad input)

**Total new: 19 passing tests.** Binary builds cleanly and runs a 15s
dry-run smoke test against real Binance successfully.
