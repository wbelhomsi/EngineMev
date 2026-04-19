# Re-architecture Audit: Chain-Agnostic EngineMev

**Date:** 2026-04-19
**Author:** Audit pass over `/home/lunatic/projects/EngineMev/`
**Question:** How much does it cost to add EVM support without forking the codebase?

---

## Executive Summary

**NO — not worth doing now.** The two real pain points (sub-ms latency on Solana
and a CLOB market-making scaffold that isn't landing yet) dwarf the cost of a
clean abstraction, and EVM MEV is a fundamentally different game (priority gas
auction / MEV-Share hints vs. post-mempool state observation) that an abstract
`ChainBackend` trait would paper over rather than solve. Keep EVM as a *separate
binary under the same Cargo workspace*, share only the three modules that are
already chain-agnostic (`router::dex::*` math, `cexdex::units/inventory/stats`,
`feed::binance`), and revisit a unified trait only after the EVM prototype
exists and its shape is known.

---

## Current Module Split (~11k LOC Rust in `src/`)

| Module | LOC | Chain-specific? | Notes |
|---|---:|:---:|---|
| `router/dex/*` (quoting math) | ~950 | **NO** | Pure u64/u128 arithmetic. Zero `Pubkey` imports. Directly reusable for Uniswap v2/v3 CPMM + CLMM math is identical. |
| `cexdex/units.rs` | 62 | **NO** | Generic lamport/atom/bps conversions, just renames needed (SOL→ETH, USDC unchanged). |
| `cexdex/inventory.rs` | 230 | **NO** | AtomicU64 balance tracking, ratio gates — zero Solana types. |
| `cexdex/stats.rs` | 235 | **NO** | JSONL writer, summary aggregation. |
| `cexdex/detector.rs` | 373 | partly | Logic is generic; struct carries `Pubkey` pool IDs — swap for a generic `PoolId` newtype. |
| `cexdex/simulator.rs` | 209 | partly | Same — CEX-priced math is generic, a few `lamports`/`atoms` naming artifacts. |
| `feed/binance.rs` + `feed/mod.rs` | 208 | **NO** | WS client + `PriceSnapshot`. Entirely reusable. |
| `metrics/*` | — | **NO** | Prometheus + OTLP, label-per-chain works fine. |
| `state/cache.rs` | 252 | **YES** | `DashMap<PoolKey{address: Pubkey}, …>`, `pair_to_pools: DashMap<(Pubkey,Pubkey), …>`. Needs generic key. |
| `state/blockhash.rs` | 142 | **YES** | `solana_sdk::hash::Hash` — EVM uses latest block number + EIP-1559 `maxFeePerGas` instead. Different concept. |
| `state/tip_floor.rs` | 401 | **YES** | Jito WS only. EVM priority fee oracle is unrelated code. |
| `state/bootstrap.rs` | 5 | — | Stub. |
| `router/pool.rs` | 296 | **YES** | `PoolState`, `RouteHop`, `ArbRoute` all hold `Pubkey` and `u64` reserves. The *dispatcher* (`get_output_amount_with_cache`) is generic enough; the struct fields leak Solana. |
| `router/calculator.rs` | 367 | partly | Logic is generic graph search; `find_routes_for_base(&Pubkey, &Pubkey)` signature needs parameterization. |
| `router/simulator.rs` | 237 | partly | `min_profit_lamports`, `tip_lamports` naming — rename to `native_atoms`. |
| `mempool/stream.rs` | 1129 | **YES** | LaserStream gRPC, per-DEX data-size routing — 100% Solana. |
| `mempool/parsers/*` | ~900 | **YES** | Each file parses one Solana account layout. EVM equivalent would be log-topic decoders. |
| `executor/bundle.rs` | 537 | **YES** | `Instruction`, `Keypair`, `Pubkey`, borsh — 100% Solana. |
| `executor/swaps/*` | ~1400 | **YES** | Per-DEX `build_*_swap_ix()` — Solana instruction builders. |
| `executor/relays/*` | ~1700 | **YES** | Jito/Astralane/Nozomi/bloXroute/ZeroSlot — all Solana relays. Trait already exists (`Relay`) but carries `&[Instruction]`, `Keypair`, `Hash`. |
| `executor/confirmation.rs` | 679 | **YES** | `getBundleStatuses`, Solana signature tracking. |
| `executor/relay_dispatcher.rs` | 136 | **YES** | Coordinates Solana relays. |
| `addresses.rs`, `sanctum.rs`, `rpc_helpers.rs` | ~500 | **YES** | Solana program IDs and RPC helpers. |
| `config.rs` | — | **YES** | Reads `SEARCHER_KEYPAIR`, `RPC_URL`, Jito endpoints. |
| `mm/*` (Manifest MM scaffold) | ~400 | **YES** | Manifest CLOB specific. |

**Summary:** roughly **2,000 LOC genuinely chain-agnostic** (18% of codebase),
**~7,500 LOC explicitly Solana-bound** (67%), **~1,500 LOC partly mixed** (15%).

---

## Proposed `ChainBackend` Trait (sketch only — see caveats below)

```rust
/// Per-chain types bundle. Everything that varies by chain lives here.
pub trait ChainTypes: 'static + Send + Sync {
    type Address: Copy + Eq + Hash + Debug;            // Pubkey on Solana, H160 on EVM
    type TxId:    Copy + Eq + Hash + Debug;            // Signature / H256
    type Signer:  Send + Sync;                         // Keypair / LocalWallet
    type Instruction: Send + Sync;                     // Instruction / TxRequest
    type NativeAmount: Copy + Ord;                     // lamports / wei (both u64/u128)
    type BlockRef: Copy;                               // recent_blockhash / block_number
    type QuoteToken: Copy + Eq + Hash;                 // mint / token_address
}

#[async_trait]
pub trait ChainBackend: Send + Sync {
    type Types: ChainTypes;

    async fn subscribe_state_changes(
        &self,
        filter: StateFilter<Self::Types>,
    ) -> Result<mpsc::Receiver<PoolStateChange<Self::Types>>>;

    async fn latest_block_ref(&self) -> Result<<Self::Types as ChainTypes>::BlockRef>;
    async fn get_nonce(&self, signer: &<Self::Types as ChainTypes>::Signer) -> Result<u64>;
    async fn priority_fee_oracle(&self) -> Result<PriorityFee<Self::Types>>;

    fn build_arb_tx(
        &self,
        route: &GenericArbRoute<Self::Types>,
        signer: &<Self::Types as ChainTypes>::Signer,
        block_ref: <Self::Types as ChainTypes>::BlockRef,
        min_output: <Self::Types as ChainTypes>::NativeAmount,
    ) -> Result<Vec<<Self::Types as ChainTypes>::Instruction>>;

    async fn submit(
        &self,
        txs: &[<Self::Types as ChainTypes>::Instruction],
        relays: &[Box<dyn Relay<Self::Types>>],
    ) -> Vec<RelaySubmitResult<Self::Types>>;
}
```

**Caveat:** this trait is *wide*. Eight associated types, every struct
(`PoolState`, `ArbRoute`, `DetectedSwap`, `StateCache`) becomes generic-over-T,
every call site grows `<B: ChainBackend>` bounds. On a 3-person codebase this
adds persistent cognitive tax for a feature (EVM) that isn't built yet.

---

## Migration Steps (ordered; LOC = delta, not touched)

1. **Extract chain-agnostic crates — easy wins.** Move `router/dex/*`,
   `cexdex/units.rs`, `cexdex/inventory.rs`, `cexdex/stats.rs`, `feed/*`,
   `metrics/*` into a `shared/` workspace crate. ~2,000 LOC *move*, ~0 LOC
   *rewrite*. **~1 day.**
2. **Newtype `PoolId` / `TokenId`.** Replace raw `Pubkey` in
   `router::pool::PoolState`, `RouteHop`, `ArbRoute`, `DetectedSwap` with a
   `#[cfg]`-gated alias (`type PoolId = Pubkey` today, `H160` tomorrow).
   ~150 LOC touched. **~0.5 day.**
3. **Abstract `StateCache`.** Make `PoolKey` generic. `pools_for_pair` keying
   already normalizes — works unchanged for `H160`. ~250 LOC touched.
   **~1 day.**
4. **Abstract router `find_routes_for_base`.** Take `&PoolId` generically.
   ~100 LOC touched. **~0.5 day.**
5. **Define `ChainBackend` + port Solana as first impl.** New trait (~200
   LOC), reorganize `main.rs` to own a `Box<dyn ChainBackend>`. `BundleBuilder`,
   `RelayDispatcher`, `GeyserStream` all get wrapped behind it without
   semantic change. **~3 days surgery + 2 days test triage.**
6. **EVM backend — net-new code.** Probably 3,000–5,000 new LOC (see gaps
   below). **~4–6 weeks.**

**Steps 1–2 alone capture 80% of the hoped-for sharing** and cost under 2
days. That's the *minimum viable refactor*. Steps 3–5 are only worth doing
if EVM is actually being built concurrently, otherwise they accrue maintenance
cost on code that may never get its second consumer.

---

## EVM-Side Gaps (no Solana code to copy)

- **Pool discovery**: EVM has no Geyser equivalent. Either poll logs via
  `eth_getLogs`, subscribe to `logs` WS, or (for private flow) subscribe to
  MEV-Share hints. Per-pool ABI decoders replace per-DEX account parsers.
- **Pricing**: Uniswap v3 tick math *is* the same u128 arithmetic we already
  have in `clmm_raydium.rs` — one of the biggest free wins. v2/v4/Curve/Balancer
  each need new modules.
- **Tx construction**: EIP-1559 transaction with `maxFeePerGas` /
  `maxPriorityFeePerGas`; the arb-guard equivalent is a Solidity router
  contract (already in the STRATEGY-MEVSHARE-ETH doc, not written).
- **Nonce management**: EVM nonces are sequential per-EOA, not durable. Replace
  `src/cexdex/nonce.rs` (280 LOC of Solana DurableNonce fan-out) with a
  sequential nonce manager + replacement-by-fee logic.
- **Relays**: Flashbots `eth_sendBundle`, bloXroute EVM, MEV-Share, bundle
  simulation via `eth_callBundle`. The `Relay` trait shape transfers; bodies
  are all-new.
- **Gas estimation**: Pre-sim every bundle with `debug_traceCall` or
  `eth_callBundle` to price gas *before* setting priority fee. No Solana
  analog (we just guess-and-check compute units).
- **Reorg handling**: EVM reorgs are real. Current code assumes final once
  observed. Needs a `block_depth >= N` wait before realizing P&L.

---

## Halal Compliance Check (reminder, non-negotiable)

Any EVM strategy candidate must pass the same bar as Solana strategies:
- **OK:** Spot DEX arb (Uniswap/Curve/Balancer), JIT LP on spot pools,
  MEV-Share backruns of user swaps, CEX↔DEX spot arb.
- **NOT OK:** Aave/Compound/Maker liquidations, flash-loan arb where the
  flash loan carries interest (even 0 bps — the *mechanism* is riba),
  leveraged perps, sandwiching, sniping.
- **Flash loans**: even when fee is zero (Balancer, dYdX-style), the transaction
  creates a debt obligation within the block. Several scholars rule this out;
  some permit it if zero-fee and atomic. Default stance for this repo: **no
  flash loans.** Use owned inventory, same as on Solana.
- MEV-Share backruns are fine — they observe a user's published hint and
  append a trade; no lending semantics involved.

---

## Bottom Line

- Worth doing **now, cheaply:** steps 1–2 (move shared crates, newtype
  `PoolId`). Captures real sharing (~2k LOC), ~1.5 days, no architectural bet.
- Worth doing **only when EVM work starts:** steps 3–5. The trait surface is
  genuinely wide and designing it pre-EVM is speculative.
- Writing EVM itself is ~4–6 weeks regardless of how clean the Solana side is.
  The abstraction doesn't shorten that.
