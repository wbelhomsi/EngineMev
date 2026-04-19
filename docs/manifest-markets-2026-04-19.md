# Manifest Market Discovery Snapshot — 2026-04-19

Output of `cargo run --release --bin manifest_discover` taken 07:13 UTC.

Scanned 1285 Manifest program accounts via `getProgramAccounts`
(256-byte header dataSlice). 25 passed the halal allowlist filter
(both base and quote mints in: USDC, USDT, PYUSD, SOL, jitoSOL, mSOL,
bSOL, JupSOL, INF, bonkSOL).

Depth is the raw vault balance (base_depth in base tokens, quote_depth
in quote tokens). Best bid/ask are UI prices decoded from the Manifest
D18 fixed-point layout; `-` means empty book side.

## Halal-compatible markets (25), sorted by quote depth

| Market | Base | Quote | Base depth | Quote depth | Best bid | Best ask |
|---|---|---|---|---|---|---|
| GVJfHJsvrsWZmVj2JVQ3KyY1n7azyi8Z2FdBPxucUe58 | PYUSD | USDC | 1,565,205 | 1,421,382 | 1.0000 | 1.0000 |
| 8sjV1AqBFvFuADBCQHhotaRq5DFFYSjjg1jMyVWMqXvZ | USDT | USDC | 296,311 | 600,785 | 1.0006 | 1.0006 |
| ENhU8LsaR7vDD2G1CsWcsuSGNrih9Cv5WZEk7q9kPapQ | SOL | USDC | 207.34 | 21,819 | 85.16 | 85.33 |
| hLwUkiJvtnThmNZeJgGsaB1zw2gJSiREoVbdEbgTEPA | JupSOL | USDC | 84.33 | 4,615 | 95.00 | 106.95 |
| 39jiBmPgZQcC1njweTyZfrRr4e8iPJYFRgL8fNTpUbaq | jitoSOL | USDC | 0.79 | 315 | 90.00 | 135.65 |
| AxSHFMAZbY3gEcKzzmweFZHxCAxCFtZruRDdS1WouLDL | mSOL | SOL | 499.59 | 207.10 | 1.3721 | 1.3722 |
| 7ecvmhGKVcK4SgxeGQJG6yVwVAhbQxLrBuaMoUmpRZ6i | jitoSOL | SOL | 3,909.90 | 202.21 | 1.2729 | 1.2730 |
| 8iC3HzYGW6ji6chaxRvNoBeG3uLgQZUPNL5R7RmM8uQv | JupSOL | SOL | 495.26 | 132.67 | 1.1809 | 1.1810 |
| BCn7bK9AURs4dVunxgRjjBnn6kGwGjEc1v7dQDrSQY88 | SOL | USDT | 0.53 | 49.99 | 80.00 | 95.00 |
| 7SanaZyHJVdcr56szAJb1Vezsi491bqs4sU8bLLef1YG | bSOL | SOL | 503.02 | 45.89 | 1.2880 | 1.2880 |
| 45n5FWcRPKoZX23TrasCLY7EdqTArkrCHSKnQb1d9vAw | jitoSOL | USDT | 0.15 | 36.51 | 105.00 | 115.00 |
| 6q5qNNuEm8dAnzW5H6z1TrbzhXEUcGosrCmJb7pVLUPm | SOL | USDC | 1.23 | 28.51 | 80.00 | 92.00 |
| GNZEf3uE87tacXe13xKzEUi5ZiZu4avifwQa3vz5ZyPs | INF | SOL | 47.25 | 1.62 | 1.4127 | 1.4175 |
| 9SaSj5c3wtWv17j8yFseMhQs5Ptmvj1X283D3UZQLe5j | USDC | SOL | 22.47 | 1.17 | 0.0095 | 0.0169 |

(remaining 11 markets have zero depth on at least one side; see
`/tmp/manifest_markets.json` for the full dump.)

## Assessment for passive MM

**Stablecoin pairs** (top 2): deepest liquidity but zero spread for us
to capture. USDT/USDC is 0.6 bps total spread — already priced by
arb bots.

**LST/SOL pairs** (rows 6–8, 10): 1-bp spreads across jitoSOL/SOL,
mSOL/SOL, JupSOL/SOL, bSOL/SOL. A competent MM is already quoting
these. We would be stepping into a price war, not providing liquidity
to an under-served pair. Getting filled at our quote means we were
mispriced relative to the fair value.

**`SOL/USDC` at `ENhU8Lsa…`**: 20-bp spread on $21K of quote depth.
This is the interesting anomaly — there is a real spread to capture,
but the depth is so small ($21K) that we would effectively *be* the
market, not an MM adding to existing liquidity. Quoting here means
our book is the book, and any taker is either:
  - A retail user who can't hit Jupiter for routing (unlikely), or
  - Someone directly arbing us (the dangerous case).

**Markets with extreme spreads** (JupSOL/USDC at 95/106.95, jitoSOL/USDC
at 90/135) are almost certainly stale orders that have been sitting
for days — not live MM activity. Quoting into these is effectively
a free-money scalp for the first actor who notices price changes,
but real flow is zero.

## Recommendation

For manifest_mm v1 LIVE, **none of these markets are an obvious fit
for passive inventory-based MM at retail scale**:

1. Deep pairs (stables) have no spread.
2. Tight LST/SOL markets are already professionally MM'd.
3. The wide-spread SOL/USDC market has no real taker flow.

The Manifest MM hypothesis ("edge is quoting quality, not microseconds")
may still hold, but it requires either:
  - A new market we seed ourselves (no existing takers = no fills), or
  - Joining LST/SOL and accepting we are competing with pro MMs (edge
    is tiny, inventory risk is real during LST rate updates), or
  - Waiting for Manifest adoption to produce bigger/wider markets.

**Next step before committing capital:** capture a second snapshot
after the Monday US open to see if daily taker flow materializes,
and run a longer `manifest_mm` dry-run against `7ecvmhGK…` (jitoSOL/SOL,
the single most-live halal market) so we see whether our quoter sits
inside or outside the current tight book.
