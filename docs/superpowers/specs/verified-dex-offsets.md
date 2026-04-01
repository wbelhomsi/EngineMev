# Verified DEX Account Offsets and Quoting Math

Verified against a production Solana trading system. These are authoritative.

## Raydium AMM v4 (752 bytes, no discriminator)

24 sequential u64 fields (offsets 0-191), then:
- `baseNeedTakePnl`: offset 192, u64
- `quoteNeedTakePnl`: offset 200, u64
- Pubkeys from offset 336: baseVault, quoteVault, baseMint, quoteMint (each 32B)

**Quoting:** Pure constant product on raw vault balances. Need_take_pnl NOT used for quoting.

## Raydium CP (637 bytes, Anchor)

Discriminator: `[247, 237, 227, 245, 215, 195, 222, 70]`

**Quoting:** `effectiveReserve = max(vaultBalance - protocolFees - fundFees - creatorFees, 0)`. Then constant product.

## Meteora DAMM v2 (1112 bytes, Anchor)

Discriminator: `[241, 154, 109, 4, 17, 177, 109, 188]`

**Two modes based on `collectFeeMode` (offset 484):**
- Mode 4 (compounding): Read `tokenAAmount`/`tokenBAmount` at offsets 680/688. Constant product.
- Mode 0-3 (concentrated): Use `liquidity` (360), `sqrtPrice` (456), `sqrtMinPrice` (424), `sqrtMaxPrice` (440). Dynamic CLMM math.

## CLMM Math (Orca Whirlpool + Raydium CLMM)

All use Q64.64 fixed-point sqrt prices:

```
deltaA = (liquidity * (sqrtPriceUpper - sqrtPriceLower)) << 64 / (sqrtPriceLower * sqrtPriceUpper)
deltaB = liquidity * (sqrtPriceUpper - sqrtPriceLower) >> 64
nextSqrtPrice (A input) = (liquidity * sqrtPrice << 64) / (liquidity << 64 + amount * sqrtPrice)
nextSqrtPrice (B input) = sqrtPrice + (amount << 64) / liquidity
```

Swap loop: find next initialized tick → compute max at this tick → if input exceeds, drain tick, update liquidity by liquidityNet, move to next → repeat.

Orca: 88 ticks per array, linear search.
Raydium CLMM: 60 ticks per array, bitmap search.

**Requires tick array accounts (separate from pool state).** Not available from pool state alone.

## Meteora DLMM Bin Math

```
price_per_bin = (1 + binStep / 10000) ^ binId
```

Per-bin swap (X for Y):
```
outAmount = (inAmount * bin.price) >> 64
maxAmountIn = ceil((bin.amountY << 64) / bin.price)
```

Per-bin swap (Y for X):
```
outAmount = (inAmount << 64) / bin.price
maxAmountIn = ceil((bin.amountX * bin.price) >> 64)
```

Dynamic fees:
```
baseFee = baseFactor × binStep × 10 × 10^baseFeePowerFactor
variableFee = ceil(variableFeeControl × (volatilityAccumulator × binStep)² / 10^11)
totalFee = min(baseFee + variableFee, 10^8)
feeOnAmount = ceil((amount × totalFee) / (10^9 - totalFee))
```

**Requires bin array accounts (separate from pool state).** Ceiling division on fees.
