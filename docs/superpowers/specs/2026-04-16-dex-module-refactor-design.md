# DEX Module Refactor — Per-DEX File Split

**Date:** 2026-04-16
**Status:** Approved
**Type:** Pure refactor — zero behavior change

## Problem

Three monolith files contain all DEX-specific logic:

| File | Lines | Contains |
|------|-------|----------|
| `router/pool.rs` | 891 | PoolState types + quoting math for 10 DEXes |
| `executor/bundle.rs` | 1,322 | BundleBuilder + swap IX builders for 10 DEXes |
| `mempool/stream.rs` | 1,710 | GeyserStream + pool state parsers for 10 DEXes |

Adding a new DEX requires editing all three files. Each file mixes shared logic with per-DEX logic. Hard to navigate, hard to review.

## Solution

Split into per-DEX files following the `executor/relays/` pattern. Each module gets a `mod.rs` dispatcher and one file per DEX. Shared math/helpers go in `mod.rs` (or a `common.rs` if large enough).

## Target Structure

### 1. `router/dex/` — Quoting Math

Extracted from `pool.rs` methods: `get_output_amount()`, `get_output_amount_with_cache()`, `get_clmm_multi_tick_output()`, `get_dlmm_bin_output()`.

```
router/dex/
├── mod.rs              # Shared: ceil_div, cpmm_output (constant product formula),
│                       #   tick_index_to_sqrt_price_x64, compute_swap_step
├── cpmm.rs             # Constant product: used by RaydiumAmm, RaydiumCp, PumpSwap, DammV2 flat
├── clmm_orca.rs        # Orca Whirlpool multi-tick crossing
├── clmm_raydium.rs     # Raydium CLMM multi-tick crossing
├── dlmm.rs             # Meteora DLMM bin-by-bin simulation
├── damm_v2.rs          # Meteora DAMM v2 concentrated
├── sanctum.rs          # LST rate-based quoting
├── phoenix.rs          # Phoenix orderbook quoting
└── manifest.rs         # Manifest orderbook quoting
```

**What stays in `pool.rs`:**
- `PoolState` struct and its non-quoting methods (`is_a_to_b`, `other_token`, etc.)
- `DexType` enum and `impl DexType` (base_fee_bps, etc.)
- `ArbRoute`, `RouteHop`, `DetectedSwap`
- `PoolExtra`, `ClmmTick`, `ClmmTickArray`, `DlmmBin`, `DlmmBinArray`

**How `PoolState::get_output_amount_with_cache()` changes:**
Becomes a thin dispatcher that calls into `dex::*` based on `self.dex_type`:
```rust
pub fn get_output_amount_with_cache(&self, ...) -> Option<u64> {
    match self.dex_type {
        DexType::MeteoraDlmm => dex::dlmm::quote(self, input_amount, a_to_b, bin_arrays),
        DexType::OrcaWhirlpool => dex::clmm_orca::quote(self, input_amount, a_to_b, tick_arrays),
        DexType::RaydiumClmm => dex::clmm_raydium::quote(self, input_amount, a_to_b, tick_arrays),
        _ => dex::cpmm::quote(self, input_amount, a_to_b),
    }
}
```

**Shared math in `dex/mod.rs`:**
- `ceil_div(a: u128, b: u128) -> u128` — used by CLMM and DLMM
- `cpmm_output(reserve_in, reserve_out, input_amount, fee_bps) -> Option<u64>` — constant product formula used by 4+ DEX types
- `tick_index_to_sqrt_price_x64(tick: i32) -> Option<u128>` — used by Orca and Raydium CLMM
- `compute_swap_step(sqrt_price, target_sqrt_price, liquidity, amount_remaining, fee_rate) -> SwapStepResult` — used by Orca and Raydium CLMM

### 2. `mempool/parsers/` — Geyser Pool State Parsers

Extracted from `stream.rs` functions: `parse_orca_whirlpool()`, `parse_raydium_clmm()`, etc.

```
mempool/parsers/
├── mod.rs              # Shared: approx_reserves_from_sqrt_price, floor_div,
│                       #   parse_by_size() dispatcher, orderbook helpers
├── orca.rs             # parse_orca_whirlpool()
├── raydium_amm.rs      # parse_raydium_amm_v4()
├── raydium_cp.rs       # parse_raydium_cp()
├── raydium_clmm.rs     # parse_raydium_clmm()
├── meteora_dlmm.rs     # parse_meteora_dlmm()
├── meteora_damm_v2.rs  # parse_meteora_damm_v2()
├── phoenix.rs          # Phoenix market parser
├── manifest.rs         # Manifest market parser
└── pumpswap.rs         # parse_pumpswap()
```

**What stays in `stream.rs`:**
- `GeyserStream` struct and `run()` method
- `PoolStateChange` struct
- LaserStream subscription logic (program filter setup, reconnection)
- Data-size routing that calls `parsers::parse_by_size(data.len(), ...)`
- Lazy vault/bin-array/tick-array fetch logic (RPC calls)
- Program owner → DEX type mapping

**Shared helpers in `parsers/mod.rs`:**
- `approx_reserves_from_sqrt_price(sqrt_price_x64, liquidity) -> (u64, u64)` — used by Orca and Raydium CLMM parsers
- `floor_div(dividend: i32, divisor: i32) -> i32` — tick array index calculation
- `parse_by_size(data_len, pool_address, data, slot) -> Option<PoolState>` — size-based dispatcher
- Shared orderbook parsing helpers for Phoenix/Manifest (`phoenix_tree_best`, etc.)

### 3. `executor/swaps/` — Swap IX Builders

Extracted from `bundle.rs` functions: `build_raydium_amm_swap_ix()`, `build_orca_whirlpool_swap_ix()`, etc.

```
executor/swaps/
├── mod.rs              # build_swap_ix() dispatcher by DexType
├── raydium_amm.rs      # build_raydium_amm_swap_ix()
├── raydium_cp.rs       # build_raydium_cp_swap_ix()
├── raydium_clmm.rs     # build_raydium_clmm_swap_ix()
├── orca.rs             # build_orca_whirlpool_swap_ix()
├── meteora_dlmm.rs     # build_meteora_dlmm_swap_ix()
├── meteora_damm_v2.rs  # build_damm_v2_swap_ix()
├── sanctum.rs          # build_sanctum_swap_ix() + sanctum_swap_accounts_v2()
├── phoenix.rs          # build_phoenix_swap_ix()
├── manifest.rs         # build_manifest_swap_ix()
└── pumpswap.rs         # build_pumpswap_swap_ix()
```

**What stays in `bundle.rs`:**
- `BundleBuilder` struct
- `build_arb_instructions()` — orchestration (compute budget, ATA creates, wSOL wrap/unwrap)
- `build_execute_arb_v2_ix()` — arb-guard CPI builder
- `build_swap_instruction()` — becomes a one-liner dispatch to `swaps::build_swap_ix()`
- `ArbV2Params`, `HopV2Params` — Borsh structs for arb-guard
- `derive_ata()`, `derive_ata_with_program()` — stay in bundle.rs (used by BundleBuilder methods)
- `estimate_unique_accounts()` — tx size estimation

## Constraints

- **Zero behavior change.** Every function keeps its exact signature and logic.
- **All 242 tests pass** without modification (functions re-exported via mod.rs).
- **No new dependencies.** Just file moves + use imports.
- **Public API unchanged.** `solana_mev_bot::router::pool::PoolState` etc. still accessible.
- **Incremental.** Can be done one module at a time (swaps first, then parsers, then dex quoting).

## Testing Strategy

- Run `cargo test` after each module split — all 242 tests must pass.
- Run `cargo clippy` — zero warnings.
- Run engine for 2 minutes after full refactor — verify identical log output pattern.

## Order of Operations

1. `executor/swaps/` first — cleanest extraction, each IX builder is fully self-contained
2. `mempool/parsers/` second — parsers are self-contained but share a few helpers
3. `router/dex/` last — quoting math has the most cross-references and shared state
