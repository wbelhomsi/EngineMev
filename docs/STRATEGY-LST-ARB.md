# Strategy: LST Rate Arbitrage

## Overview

Liquid staking tokens (jitoSOL, mSOL, bSOL) accrue staking rewards continuously, causing their SOL-denominated exchange rate to drift. Different DEX pools update their reserves at different speeds. When jitoSOL/SOL is 1.080 on Raydium but 1.082 on Orca, buy cheap on Raydium and sell expensive on Orca in a single atomic bundle.

Halal: pure spot arbitrage between fungible assets. No borrowing, no leverage.

## Why This Is the Easiest Win

This is literally the same arb EngineMev already does — just with LST tokens added to the monitored set. The infrastructure is identical:

1. Add LST vault addresses to Geyser subscription
2. Add LST pools to state cache
3. Existing RouteCalculator already finds 2-hop circular routes
4. Existing simulator and bundle pipeline handles execution

**Estimated new code: ~50 lines** (config additions + LST mint/pool registration).

## LST Token Addresses (Verified)

| Token   | Mint Address                                          | Stake Pool Program         |
|---------|-------------------------------------------------------|----------------------------|
| jitoSOL | `J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn`     | SPL Stake Pool             |
| mSOL    | `mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So`      | Marinade Finance           |
| bSOL    | `bSo13r4TkiE4KumL71LsHTPpL2euBYLFx6h9HP3piy1`      | BlazeStake                 |
| scnSOL  | `5oVNBeEEQvYi1cX3ir8Dx5n1P7pdxydbGF2X4TxVusJm`     | Socean                     |
| stSOL   | `7dHbWXmci3dT8UFYWYZweBLXgycu7Y3iL6trKn1Y7ARj`     | Lido (deprecated but pools exist) |
| JSOL    | `7Q2afV64in6N6SeZsAAB81TJzwDoD6zpqmHkzi9Dcavn`     | JPOOL                      |

## Why LSTs Create Persistent Arb Opportunities

1. **Rate accrual is asynchronous**: Staking rewards compound per epoch (~2 days). Different stake pools update their rate at different times.
2. **Pool rebalancing lag**: When jitoSOL's exchange rate ticks up, Raydium's pool doesn't instantly reflect it. Someone has to swap to move the reserves.
3. **Multiple DEXes, same pair**: jitoSOL/SOL pools exist on Raydium, Orca, Meteora, and Jupiter. Each has independent reserves.
4. **Sanctum router**: Sanctum provides instant LST→SOL conversion at oracle rate, creating a baseline price. Pools that deviate from Sanctum's rate are arbitrageable.

## Architecture

Minimal additions to existing EngineMev:

```
Existing pipeline (unchanged):
  Geyser → PoolStateChange → StateCache → RouteCalculator → Simulator → Bundle → Relay

New additions:
  1. config.rs: Add LST mint addresses
  2. Pool bootstrapping: Include LST/SOL and LST/LST pools in initial getProgramAccounts
  3. StateCache: Register LST pool vaults during bootstrap
```

## Key Pools to Monitor

### jitoSOL/SOL Pools
- Raydium AMM: High volume, constant-product
- Orca Whirlpool: Concentrated liquidity, tighter spreads
- Meteora DLMM: Dynamic bins, auto-rebalancing

### mSOL/SOL Pools
- Raydium AMM
- Orca Whirlpool
- Marinade native unstake (delayed but exact rate)

### Cross-LST Pools
- jitoSOL/mSOL on Orca — most interesting because BOTH sides accrue at different rates
- bSOL/SOL on Raydium

## Route Examples

```
2-hop: SOL → jitoSOL (Raydium, rate 1.080) → SOL (Orca, rate 1.082) = +0.185% profit
2-hop: mSOL → SOL (Raydium) → mSOL (Orca) = capture rate difference
3-hop: SOL → jitoSOL (Raydium) → mSOL (Meteora) → SOL (Orca) = triangle arb
```

## Config Additions

```rust
// In config.rs — add to monitored_programs() or as separate LST config
pub fn lst_mints() -> Vec<(Pubkey, &'static str)> {
    vec![
        (Pubkey::from_str("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn").unwrap(), "jitoSOL"),
        (Pubkey::from_str("mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So").unwrap(), "mSOL"),
        (Pubkey::from_str("bSo13r4TkiE4KumL71LsHTPpL2euBYLFx6h9HP3piy1").unwrap(), "bSOL"),
    ]
}
```

```env
# LST Arb
LST_ARB_ENABLED=true
LST_MIN_SPREAD_BPS=5    # LST spreads are thin — 5 bps minimum
```

## Risk Profile

**Very low risk:**
- LSTs are backed 1:1+ by staked SOL — they don't go to zero
- Arb is atomic (single Jito bundle) — no partial execution
- No impermanent loss — we're not LPing, just swapping
- Worst case: bundle doesn't land, we pay nothing

**Only risk:** Stale pool state → miscalculated profit → bundle reverts → tip wasted. Mitigated by simulator re-reading fresh state from cache.

## Estimated Profitability

- LST rate spreads: typically 2-20 bps across pools
- After tip: 1-15 bps net
- jitoSOL daily volume across all pools: ~$5-15M
- If we capture 0.1% of flow at 5 bps: $25-75/day
- Scales linearly with more LST pairs and pools

## Implementation Steps

1. Add LST mint addresses to `config.rs`
2. During pool bootstrap (TODO), include LST/SOL pool accounts
3. Register LST pool vaults in StateCache vault→pool index
4. Existing RouteCalculator + Simulator handles the rest
5. Test in DRY_RUN, verify route detection on real Geyser data
