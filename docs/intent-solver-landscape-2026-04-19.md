# Solana Intent/RFQ Solver Landscape — 2026-04-19

Research briefing on how a retail developer can onboard as a solver /
filler on Solana intent protocols. Relevant because the "edge is quoting
quality, not microseconds" thesis motivated the Manifest MM pivot — and
intent/RFQ is the direct expression of that thesis at protocol scale.

## TL;DR (ranked by retail feasibility)

1. **Pyth Express Relay (powers Kamino Swap RFQ)** — explicitly permissionless,
   public TS/Python SDKs, auction model. **Best entry point.**
2. **DFlow filler** — docs say "entirely permissionless"; webhook model,
   uses Helius LaserStream under the hood (our Frankfurt colo already fits).
3. **JupiterZ / Hashflow / Titan** — effectively whitelisted, dominated by
   prop firms (HumidiFi, Tessera/Wintermute, SolFi). Don't waste cycles.

## Venue-by-venue summary

### Pyth Express Relay (recommended first step)

- **Onboarding:** Permissionless. "A few lines of code." No sales call.
- **Docs:** <https://docs.pyth.network/express-relay/integrate-as-searcher>
  Code: <https://github.com/pyth-network/per>
- **Revenue:** Auction-based. Searcher bids for the right to execute;
  captures (opportunity_value − bid). Losers pay nothing.
- **Tech:** Subscribe to off-chain auction stream, submit signed bids.
  No special RPC required, any keypair works.
- **Flow types seen:** Kamino Swap RFQ, limit-order fills, liquidations
  on integrated protocols, cross-DEX arb.
- **Existing searchers (public list):** Amber, Auros, Caladan, Flowdesk,
  Selini, Tokka, Wintermute. Pros are there but the venue is open.
- **⚠️ HALAL RED FLAG:** Express Relay also auctions **liquidation
  opportunities** on lending protocols (Kamino lend, MarginFi). We
  forbid liquidations per CLAUDE.md. The opportunity type is in the
  auction payload — **our searcher must filter out `liquidation_*` and
  bid only on swap/arb auctions**. This is non-negotiable.

### DFlow filler (recommended second)

- **Onboarding:** Docs literally state "implementation is entirely
  permissionless." Dev endpoints key-free; prod needs a DFlow-issued key.
- **Docs:** <https://docs.dflow.net/docs/fill-orders-solana>
  Reference: <https://docs.dflow.net/reference/get_solana-firmquote>
  Intro: <https://pond.dflow.net/introduction>
  Community: discord.gg/dflow
- **Revenue:** Spread on fills. Paid at `/sendTransaction` time →
  incentivized to ensure settlement.
- **Tech:** Host an HTTP server returning firm quotes. Uses Helius
  LaserStream — our existing Helius integration transfers directly.
- **Flow:** Segments toxic vs non-toxic order flow (retail-biased →
  less adverse selection than open markets). Aggregator share 5–10%,
  peaked 47.9% on a single day in Nov 2025.
- **Halal:** Clean for spot fills. Their separate Kalshi prediction-
  markets product is maysir-adjacent but that's a different API; the
  filler role itself is untainted.

### Skip list

| Venue | Why skip |
|---|---|
| JupiterZ | Whitelisted; must apply via Discord. Flow ~80% consumed by prop AMMs (HumidiFi 62% of executed Jup volume). Retail solver won't win SOL/USDC quotes against them. |
| Hashflow | Explicit allowlist. Institutional. 750 ms quote budget, SECP256k1 signing — their stack is built for pro MMs. |
| Titan | Not an RFQ venue. It's a meta-aggregator consuming Jupiter/OKX/DFlow quotes. No third-party solver onboarding. |

## Meta-observations

- **Permissionless in practice?** Express Relay: yes. DFlow: yes per docs.
  JupiterZ / Hashflow: no — you apply and negotiate.
- **Liquidity share (rough):** Jupiter ~93.6% of aggregator flow on
  Solana; DFlow 5-10%; Titan meta-layer ~$35M/week.
- **Retail solver success stories?** None public. Searcher lists are
  all pro firms. Retail edge candidates:
  - Long-tail SPLs that prop-AMMs ignore
  - Inventory plays where existing CEX arb flow provides signal

## Concrete first step

**Read <https://docs.pyth.network/express-relay/integrate-as-searcher>
and clone <https://github.com/pyth-network/per>.** Write a no-op Python
searcher that:

1. Subscribes to the auction stream
2. **Filters out any opportunity with type containing `liquidation`** (halal)
3. Bids on Kamino Swap RFQs using our existing Binance SOL/USDC feed
   as the reference price

Given our stack (Binance WS, Frankfurt colo, pool-state cache), we
already have everything needed to price SOL/USDC RFQs competitively.
If this produces any fills after 1-2 weeks, escalate to DFlow filler
onboarding. Treat JupiterZ / Hashflow as Phase 3 contingent on traction
elsewhere.

## Sources

- [Jupiter RFQ Integration docs](https://developers.jup.ag/docs/routing/rfq-integration)
- [DFlow Fill orders (Solana)](https://docs.dflow.net/docs/fill-orders-solana)
- [DFlow Welcome / docs index](https://pond.dflow.net/introduction)
- [Helius blog on DFlow + LaserStream](https://www.helius.dev/blog/dflow)
- [Hashflow Market Making API v3](https://docs.hashflow.com/hashflow/market-making/getting-started-api-v3)
- [Pyth Express Relay searcher integration](https://docs.pyth.network/express-relay/integrate-as-searcher)
- [Pyth Express Relay GitHub](https://github.com/pyth-network/per)
- [Kamino Swap + Express Relay (Pyth)](https://www.pyth.network/blog/stop-overpaying-for-swaps-express-relay-on-kamino-swap)
- [Jupiter 93.6% aggregator market share](https://solanafloor.com/news/jupiter-reclaims-dominance-with-93-6-market-share-in-solana-s-aggregator-landscape)
- [Solana prop-AMM landscape (Helius)](https://www.helius.dev/blog/solanas-proprietary-amm-revolution)
- [Market making propAMMs landscape (Chorus One)](https://chorus.one/reports-research/market-making-propamms-and-solana-execution-quality-landscape)
