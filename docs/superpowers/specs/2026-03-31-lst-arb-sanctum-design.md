# Phase 2: LST Rate Arbitrage with Sanctum Virtual Pool

## Summary

Add liquid staking token (jitoSOL, mSOL, bSOL) arbitrage to EngineMev by registering LST/SOL pools from existing DEXes and modeling Sanctum Infinity as a virtual pool. The existing pipeline (Geyser -> RouteCalculator -> Simulator -> BundleBuilder -> MultiRelay) is unchanged. LST pools appear as additional `PoolState` entries in the `StateCache`, and the RouteCalculator discovers cross-DEX and DEX-to-Sanctum arb routes automatically.

Halal-compliant: pure spot arbitrage between fungible assets. No borrowing, no leverage, no lending protocol interaction.

## Motivation

LST exchange rates accrue staking rewards per epoch (~2 days). Different DEX pools update reserves at different speeds, creating persistent 2-20 bps spreads. Sanctum Infinity provides oracle-rate LST<->SOL conversion, giving us a reliable baseline to arb against. Any DEX pool that deviates from the Sanctum oracle rate is arbitrageable.

Estimated daily revenue: $5-60/day initially, scaling with more LST pairs and infrastructure speed. Near-zero marginal cost since the infrastructure already exists.

## Architecture

### No new modules

All changes bolt onto existing files:

| File | Changes |
|------|---------|
| `config.rs` | LST mint addresses, Sanctum program IDs, SOL Value Calculator mapping, `LST_MIN_SPREAD_BPS` env var |
| `pool.rs` | `DexType::SanctumInfinity` variant, `base_fee_bps()` for Sanctum |
| `cache.rs` | No structural changes (more pools registered at bootstrap) |
| `stream.rs` | Sanctum reserve ATA addresses added to Geyser subscription filter |
| `bundle.rs` | `build_sanctum_swap_ix()` with full Sanctum Infinity account layout |
| `simulator.rs` | LST spread threshold check (`LST_MIN_SPREAD_BPS`) as additional gate |
| `main.rs` | Bootstrap Sanctum virtual pools at startup, register vault ATAs |

### Data flow (unchanged pipeline, new data)

```
Geyser streams vault balance changes
  (now includes Sanctum reserve ATAs + LST DEX pool vaults)
    -> PoolStateChange on crossbeam channel
    -> StateCache.update_vault_balance() updates affected pool
    -> Router constructs DetectedSwap trigger, searches both directions
    -> RouteCalculator.find_routes() now finds routes like:
         SOL -> jitoSOL (Orca, cheap) -> SOL (Sanctum, oracle rate)
         jitoSOL -> SOL (Raydium) -> jitoSOL (Meteora)
         SOL -> jitoSOL (Raydium) -> mSOL (Orca) -> SOL (Meteora)
    -> ProfitSimulator validates with fresh state + LST spread check
    -> BundleBuilder builds arb tx (dispatches to correct IX builder per DexType)
    -> MultiRelay fan-out
```

### Sanctum virtual pool modeling

Sanctum Infinity uses oracle-based pricing (not constant-product). It converts LST amounts to intrinsic SOL value via per-LST SOL Value Calculator programs, with near-zero price impact for typical arb sizes against its ~2M SOL TVL.

We model each LST/SOL pair as a `PoolState` with `DexType::SanctumInfinity`:
- `token_a_mint` = SOL (native mint `So11111111111111111111111111111111111111112`)
- `token_b_mint` = LST mint (one virtual pool per LST)
- `token_a_reserve` / `token_b_reserve` = synthetic large values that produce the correct exchange rate under constant-product math
- `fee_bps` = Sanctum's actual fee for that LST (input_fee + output_fee, typically 1-3 bps)
- `dex_type` = `DexType::SanctumInfinity`

To encode target rate `R` (SOL per LST) in constant-product reserves:
- `SYNTHETIC_RESERVE_BASE = 1_000_000_000_000_000` (1B SOL in lamports, chosen to be >1000x any realistic arb input so price impact is <0.1%)
- `reserve_a (SOL side) = SYNTHETIC_RESERVE_BASE`
- `reserve_b (LST side) = SYNTHETIC_RESERVE_BASE / R`
- For small inputs relative to reserves, `output ~= input * R * (1 - fee)`, matching Sanctum's actual behavior
- Example: jitoSOL rate 1.082 SOL -> reserve_a = 1_000_000_000_000_000, reserve_b = 924_214_417_744_917

This approximation is accurate for arb-sized trades because Sanctum's real price impact is near-zero at these sizes. The simulator re-validates with fresh state before submission regardless.

### Bootstrap sequence (startup)

1. RPC `getProgramAccounts` for each DEX program -> populate cache with real pool states, register vault addresses
2. For each LST mint, fetch on-chain stake pool account -> read `total_lamports / pool_token_supply` for initial exchange rate
3. Create Sanctum virtual pools with synthetic reserves encoding the rate
4. Derive Sanctum reserve ATAs (from Pool State PDA + each LST mint) and register in vault->pool index
5. Add all vault addresses to Geyser subscription

## LST Token Addresses

| Token | Mint Address | Notes |
|-------|-------------|-------|
| jitoSOL | `J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn` | SPL Stake Pool, highest volume |
| mSOL | `mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So` | Marinade Finance |
| bSOL | `bSo13r4TkiE4KumL71LsHTPpL2euBYLFx6h9HP3piy1` | BlazeStake |

## Sanctum Program IDs

| Program | Address |
|---------|---------|
| S Controller (Infinity) | `5ocnV1qiCgaQR8Jb8xWnVbApfaygJ8tNoZfgPwsgx9kx` |
| Flat Fee Pricing | `f1tUoNEKrDp1oeGn4zxr7bh41eN6VcfHjfrL3ZqQday` |
| SPL SOL Value Calculator | `sp1V4h2gWorkGhVcazBc22Hfo2f5sd7jcjT4EDPrWFF` |
| SanctumSpl SOL Value Calculator | `sspUE1vrh7xRoXxGsg7vR1zde2WdGtJRbyK9uRumBDy` |
| SanctumSplMulti SOL Value Calculator | `ssmbu3KZxgonUtjEMCKspZzxvUQCxAFnyh1rcHUeEDo` |
| Marinade SOL Value Calculator | `mare3SCyfZkAndpBRBeonETmkCCB3TJTTrz8ZN2dnhP` |
| wSOL SOL Value Calculator | `wsoGmxQLSvwWpuaidCApxN5kEowLe2HLQLJhCQnj4bE` |

### SOL Value Calculator mapping (LST mint -> calculator program)

| LST Mint | Calculator |
|----------|-----------|
| jitoSOL (`J1toso...`) | SPL SOL Value Calculator (`sp1V4h...`) |
| mSOL (`mSoLz...`) | Marinade SOL Value Calculator (`mare3S...`) |
| bSOL (`bSo13r...`) | SPL SOL Value Calculator (`sp1V4h...`) |

## Sanctum SwapExactIn Account Layout

The `build_sanctum_swap_ix()` function in `bundle.rs` must provide these accounts:

```
1.  [signer]    Payer / signer (searcher keypair)
2.  [writable]  Pool State PDA          (seeds: [b"state"], program: S Controller)
3.  []          LST State List PDA      (seeds: [b"lst-state-list"], program: S Controller)
4.  [writable]  Source LST Reserve ATA  (derived: ATA(Pool State PDA, input_mint))
5.  [writable]  Dest LST Reserve ATA    (derived: ATA(Pool State PDA, output_mint))
6.  []          Pricing Program         (Flat Fee: f1tUoNEKrDp1oeGn4zxr7bh41eN6VcfHjfrL3ZqQday)
7.  []          Source SOL Value Calc   (per-LST, from mapping above)
8.  []          Dest SOL Value Calc     (per-LST, from mapping above)
9.  [writable]  User source token ATA   (signer's ATA for input mint)
10. [writable]  User dest token ATA     (signer's ATA for output mint)
11. []          Token Program           (TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA)
12. []          System Program          (11111111111111111111111111111111)
```

Instruction data: `SwapExactIn { amount: u64, minimum_amount_out: u64 }`

## Atomicity & Profit Guarantee

Hard invariant: **every landed transaction produces profit. No exceptions.**

### Bundle structure

```
Bundle = [arb_tx, tip_tx]

arb_tx (single transaction, atomic):
  IX 1: Swap on DEX (e.g., buy jitoSOL on Orca)
        minimum_amount_out = 0 (intermediate hop)
  IX 2: Swap on Sanctum (e.g., sell jitoSOL at oracle rate)
        minimum_amount_out = input_amount + min_profit_after_tip

tip_tx:
  IX 1: Transfer tip_lamports to Jito tip account
```

### Profit enforcement

The `minimum_amount_out` parameter on the **final hop** acts as the on-chain profit assertion:
- Set to `input_amount + min_profit_after_tip`
- If the route doesn't produce enough tokens, the final swap instruction reverts
- Transaction reverts -> bundle drops -> tip never paid

No custom program needed. Every DEX swap instruction (Raydium, Orca, Meteora, Sanctum) already supports `minimum_amount_out`.

### Failure modes

| Scenario | Result | Cost |
|----------|--------|------|
| Arb lands profitably | Profit captured, tip paid | None |
| Stale state / bad rate | Last swap reverts (min_out not met), bundle dropped | Zero |
| Pool state changed between sim and landing | Last swap reverts, bundle dropped | Zero |
| Jito auction lost | Bundle not included in block | Zero |
| Geyser disconnect | No events, no bundles built | Zero |

The only way to lose money would be if a tip tx lands without the arb tx. This cannot happen — Jito bundle atomicity guarantees all-or-nothing execution.

### BundleBuilder changes

`BundleBuilder::build_arb_bundle()` must:
1. Set `minimum_amount_out = 0` on all intermediate hops
2. Set `minimum_amount_out = input_amount + min_profit_after_tip` on the **last hop**
3. Place arb tx first, tip tx second in the bundle
4. Each hop dispatches to the correct IX builder based on `DexType`

## Configuration

### New environment variables

```env
# LST Arb
LST_ARB_ENABLED=true
LST_MIN_SPREAD_BPS=5    # Minimum spread in bps to consider an LST route (default: 5)
```

### Config struct additions

```rust
// In BotConfig
pub lst_arb_enabled: bool,
pub lst_min_spread_bps: u64,
```

### LST mint registry in config.rs

```rust
pub fn lst_mints() -> Vec<(Pubkey, &'static str)> {
    vec![
        (pubkey!("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn"), "jitoSOL"),
        (pubkey!("mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So"), "mSOL"),
        (pubkey!("bSo13r4TkiE4KumL71LsHTPpL2euBYLFx6h9HP3piy1"), "bSOL"),
    ]
}

pub fn sanctum_sol_value_calculator(mint: &Pubkey) -> Option<Pubkey> {
    // Returns the correct SOL Value Calculator program for a given LST mint
}
```

## Testing Strategy

### Unit tests (in-process, no RPC)

| Test | What it verifies |
|------|-----------------|
| `test_sanctum_virtual_pool_rate` | Synthetic reserves produce correct output for known exchange rates (e.g., jitoSOL at 1.082 SOL) |
| `test_sanctum_virtual_pool_fee` | Fee deduction matches Sanctum's actual fee_bps |
| `test_lst_mint_config` | All LST mint addresses parse correctly, SOL Value Calculator mapping is complete for each |
| `test_route_discovery_lst` | Mock pools in StateCache -> RouteCalculator finds SOL->jitoSOL(Orca)->SOL(Sanctum) route |
| `test_route_discovery_cross_lst` | Mock pools -> finds SOL->jitoSOL(Raydium)->mSOL(Orca)->SOL(Sanctum) 3-hop |
| `test_profit_assertion_amount` | `minimum_amount_out` on final hop equals `input_amount + min_profit_after_tip` |
| `test_sanctum_ix_accounts` | Sanctum swap instruction builds with correct accounts and serializes properly |
| `test_sanctum_ix_pda_derivation` | Pool State PDA and LST State List PDA derive to expected addresses |
| `test_simulator_lst_spread_gate` | Simulator rejects routes below `LST_MIN_SPREAD_BPS` threshold |

### Integration tests (Surfpool, mainnet fork)

| Test | What it verifies |
|------|-----------------|
| `test_real_pool_state_bootstrap` | Start Surfpool with `--network mainnet`, fetch real Raydium/Orca jitoSOL/SOL pool state, verify cache populated correctly |
| `test_manipulated_spread_detection` | Use `surfnet_setTokenAccount` to manipulate vault balances on a real jitoSOL/SOL pool, creating a known 10bps spread -> verify RouteCalculator detects the arb |
| `test_scenario_price_dislocation` | Use `surfnet_registerScenario` with Raydium/Whirlpool templates to simulate price dislocation -> verify route found |
| `test_simulator_real_pools` | Simulator approves profitable routes and rejects unprofitable ones against real pool math |
| `test_vault_update_propagation` | Vault balance change propagates through cache -> triggers route discovery |

### E2E tests (Surfpool, full pipeline)

| Test | What it verifies |
|------|-----------------|
| `test_e2e_profitable_arb` | Full pipeline: Surfpool mainnet fork -> manipulate vault balances to create spread -> inject PoolStateChange -> router finds route -> simulator approves -> BundleBuilder produces tx -> submit to Surfpool RPC -> tx lands -> profit assertion holds |
| `test_e2e_revert_unprofitable` | Set vault balances so arb is unprofitable -> verify `minimum_amount_out` causes revert -> confirm tx does not land |
| `test_e2e_cu_budget` | Use `surfnet_profileTransaction` to verify arb bundle CU usage stays within compute budget |
| `test_e2e_epoch_rate_update` | `surfnet_timeTravel` to next epoch -> LST rate ticks up -> verify rate update propagates to virtual pool -> new arb opportunity detected |
| `test_e2e_stale_state_safety` | Inject stale PoolStateChange (old slot) -> verify cache ignores it -> no false arb detected |
| `test_e2e_channel_backpressure` | Flood crossbeam channel with events -> verify `try_send` drops stale events gracefully -> router processes latest |

### CI integration

```bash
NO_DNA=1 surfpool start --ci --network mainnet &
sleep 2  # wait for surfnet ready
cargo test --features e2e -- --test-threads=1
```

Feature-gated: e2e tests behind `#[cfg(feature = "e2e")]` so `cargo test` alone runs only unit tests.

## Scope Estimate

| File | Lines (approx) |
|------|----------------|
| `config.rs` | ~40 new |
| `pool.rs` | ~5 new |
| `stream.rs` | ~10 new |
| `bundle.rs` | ~40 new |
| `simulator.rs` | ~10 new |
| `main.rs` | ~30 new |
| Unit tests | ~150 new |
| Integration tests (Surfpool) | ~100 new |
| E2E tests (Surfpool) | ~150 new |
| **Total** | ~535 lines |

## Out of Scope

- CLMM tick-crossing math (Phase 1 roadmap item, not Phase 2)
- Pool state bootstrapping via `getProgramAccounts` (Phase 1 roadmap item, reused here)
- Sanctum oracle rate polling (Approach B upgrade path if thin-spread reverts are too frequent)
- Additional LSTs beyond jitoSOL/mSOL/bSOL (trivial to add later via config)
- scnSOL, stSOL, JSOL (mentioned in strategy doc but lower volume, add after proving jitoSOL/mSOL/bSOL)
