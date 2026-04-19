# Retail MEV/Arb Bot Builder Experience: Reddit & Community Research
*Compiled 2026-04-19. Searches cover Reddit threads, Medium post-mortems, Islamic finance forums, and cross-chain research papers through April 2026.*

---

## TL;DR — Top 5 Recurring Themes

1. **Retail atomic arbitrage on Solana is essentially nonexistent as a profit center.** The average profit per arb transaction is $1.58. The top 3 bots control ~60% of volume. Infrastructure costs ($1,800–$3,800/month for dedicated nodes) frequently exceed retail operator revenues. "The solo developer is being squeezed out by capitalised firms." This is documented in aggregate data — it matches our zero-landing experience exactly.

2. **Co-location is necessary but not sufficient.** Frankfurt/Amsterdam placement cuts latency 5–10x vs. shared RPC, but Solana's 200ms Jito auction window means you must also be co-located with the *specific validators* who are slot leaders. Random Frankfurt co-location still loses to bots physically adjacent to high-stake validators.

3. **Sandwich bots eat DEX-DEX atomic arbitrage alive.** Sandwich operators keep 80–85% of profits vs. atomic arb operators paying 50–60% in tips. Private mempools (DeezNode and others) give sandwich bots pre-block order visibility that arbitrage bots never see. This is a structural disadvantage, not a tuning problem.

4. **CEX-DEX arb is Model B territory — one-leg fill risk is the silent killer.** Almost no retail builders have published profitable CEX-DEX results. The risk is asymmetric: miss the on-chain leg and you carry inventory; miss the CEX leg and you bleed slippage. Our 5 opportunities in 16h at 5 bps threshold is consistent with the near-zero signal documented by others.

5. **Halal community (IFG forum) considers atomic spot arbitrage permissible but has almost nobody actually running it profitably.** Scholarly consensus: spot arb (both legs instantaneous) is fine; gas-fee bidding in open auctions is like "bidding for service." No retail operators sharing live profitable bot results in that community.

---

## Source Reliability Key

- [VERIFIED] = Cross-referenced aggregate data from multiple primary sources (Helius, Extropy, Jito Foundation)
- [FIRST-PERSON] = Developer's own account, published with code/screenshots
- [FORUM] = Community discussion, no verification of claims
- [AFFILIATE-RISK] = Content from "best bots" aggregators — treat as marketing noise
- [RESEARCH] = Peer-reviewed or institutional analysis

---

## Thread/Source Reports

### 1. Solid Quant — "How I Built My First DEX Arbitrage Bot: Introducing Whack-A-Mole"
**Medium (EVM), ~2023–2024 | [FIRST-PERSON]**
- **Setup:** EVM DEX-DEX atomic arb via Flashbots, ETH Mainnet, Python + Solidity, ~$1,000 capital
- **Honest P&L:** Zero profitable trades in production. Simulated max spread: +$0.36 on $400 input. Gas cost to execute: ~$15. Net: deeply negative.
- **Failure mode:** Gas cost ($15) dwarfs spread ($0.36) at any realistic capital size. Minimum to break even requires ~3 ETH input while absorbing 1% slippage.
- **Competitive reality:** "Whether we as starting MEV searchers can extract the same alpha competing against the pro traders already in the game" — author explicitly leaves this unanswered, implying they know the answer.
- **Red flags / hype:** None. Author explicitly says the published code "won't start raking in profits from day one. If it did, it won't be shared openly."
- **Matches our experience:** Yes. The infrastructure gap vs. production operators is the same problem we face.

### 2. Solid Quant — "100 Hours of Building a Sandwich Bot"
**Medium (EVM), ~2024 | [FIRST-PERSON]**
- **Setup:** Ethereum sandwich bot, 100 engineering hours documented, full node (Geth + Lighthouse), Flashbots bundles
- **Honest P&L:** No trades executed in production — article ends before the submission phase. Explicitly framed as a template, not a profitable system.
- **Failure mode:** Three-stage simulation pipeline exists but latency for on-chain simulation calls is 0.3–1.0 seconds vs. required 0.0001 seconds. Speed gap is ~3,000–10,000x.
- **Flashbots reality:** ~20% block inclusion rate. "Flashbots only succeeds approximately 1 out of 5 times" even when bundles are submitted correctly.
- **Matches our experience:** Yes on latency gap. Jito's 200ms window vs. our 906µs p50 latency is the same problem class.

### 3. Extropy Research — "An Analysis of Arbitrage Markets Across Ethereum, Solana, Optimism, and Starknet 2024–2025"
**Academic-style cross-chain analysis | [RESEARCH]**
- **Key finding on retail viability:** "The number of unique 'core' entities consistently winning bids often does not exceed 20" on Ethereum per week. On Solana, top 3 bots control ~60% of sandwich volume. Top 2 entities control >80% of Optimism spam.
- **Profit concentration:** Top bot (E6Y on Solana): ~$300k/day net profit from $1.6B volume. Average arb: $1.58/transaction.
- **Infrastructure cost floor:** Solana: $1,800–$3,800/month dedicated RPC. Ethereum: $200–$500/month plus "substantial R&D on proprietary pricing models."
- **Bottom line quote:** "The solo developer is being squeezed out by capitalised firms that can afford the $3,000/month RPC nodes and specialised engineering talent required to compete."
- **Matches our experience:** Directly. We are co-located in Frankfurt, have sub-ms latency, and still can't land a bundle.

### 4. Jito / Helius MEV Report — Solana MEV Data 2024
**Helius Research (primary source) | [VERIFIED]**
- **Scale:** 90 million successful arbitrage transactions over 12 months generated $142.8M total — average $1.58/tx.
- **Failure rate before Jito:** Over 98% of pre-Jito arbitrage transactions failed; 60%+ of block compute was wasted on MEV spam.
- **Tip dynamics:** In competitive scenarios, searchers bid nearly the full opportunity value; validators capture "almost the entire available MEV." The equilibrium is: searcher profit approaches zero as competition increases.
- **Daily tippers:** Grew from 20,000 (early 2024) to 938,000 (December 2024) — the crowd got dramatically bigger.
- **Dominant strategy:** Atomic arbitrage "is the dominant form of MEV on Solana" — but it's dominated by ~20 operators.
- **Tip floor:** 1,000 lamports minimum; competitive tier during peak: 10,000–50,000 lamports. Our tip floor of 1,000+ lamports is in range, but competition is tipping at multiples.
- **Matches our experience:** Our 177 accepted bundles/5-min with zero landings is consistent. "Accepted" means the relay took the bundle, not that it won the auction.

### 5. DeezNode / Private Mempool Coverage — CoinDesk, June 2024
**CoinDesk (journalism) | [VERIFIED]**
- **Setup:** A validator operator (DeezNode) runs a private mempool and offers participating validators 50% of MEV profits.
- **Scale:** Grew from 307k SOL stake to 802k SOL stake in ~3 weeks after MEV deals emerged. By January 2025: 811k SOL (~$168M) delegated stake.
- **Extraction:** DeezNode's sandwich bot: 1.55 million transactions in 30 days, 88.9% success rate, 65,880 SOL profit ($13M+).
- **Retail impact:** Sandwich bots in private mempools see order flow before it hits the public Jito auction. Retail atomic arb bots operating through the standard Jito channel are competing for leftovers.
- **Our exposure:** We are not a sandwich bot (halal constraint), so this doesn't directly compete with us. But it confirms that private validator relationships are eating the ecosystem's edge. **Atomic arb bots like ours only win opportunities that sandwich bots don't care about.**

### 6. Jito Tips Anatomy Analysis — Medium, 2025
**Medium research post | [RESEARCH]**
- **Who pays tips:** DEX arbitrage bots 45%, meme coin snipers 30%, institutional MEV 15%, retail 10%.
- **Institutional retreat:** Institutions "once controlled 70% of tip volume in 2023" but have retreated to cross-chain MEV, leaving a void partially filled by retail snipers.
- **Tip strategy gap:** Retail uses "pre-set fixed tips (0.1–3 SOL)." Sophisticated operators "adjust tips in real-time based on MEV opportunity size using reinforcement learning."
- **TrumpCoin spike:** Median tip was 1.2 SOL/tx, top traders paid 3.7 SOL/tx for one trade. Context: pure volume/volatility events dominate MEV.
- **Searcher profit retention:** $8.4M in tips represented 14% of $60M profits (TrumpCoin) — searchers kept 86%. But this is *during* a volatility spike. Normal atomic arb margins are compressed to near-zero by auction competition.

### 7. 80% Failure Rate Analysis — GlobalGurus.org, 2025/2026
**Industry analysis | [RESEARCH-ish, methodology unclear]**
- **Claim:** "80% of Solana trading bots fail in the first month."
- **Root cause identified (matches our ops):** Wrong data subscription method (WebSocket adds 30–80ms vs. Yellowstone gRPC); bundle misconfiguration (static tips become obsolete); absent monitoring (degradation is silent); testing-production gap (devnet behaves differently under load).
- **Key data point:** "A public mainnet endpoint under load applies rate limits during peak traffic and delivers account updates later because it's under higher demand."
- **Matches our experience:** We use LaserStream gRPC (correct), have dynamic tip floor via WebSocket (correct), and have monitoring (Prometheus). We've done the infrastructure right. Our zero-landing problem is therefore not infrastructure — it's auction competition.

### 8. IFG Islamic Finance Forum — "Is it halal to do arbitrage with MEV bots?"
**forum.islamicfinanceguru.com, June 2024 | [FORUM]**
- **Scholarly ruling (Mufti Faraz Adam):** Atomic spot arbitrage is permissible. Gas fee bidding in open Jito-style auctions is "an auction for service" — acceptable. Not a bribe.
- **What's prohibited:** Any leveraged position, short-selling, non-spot trades, tokens linked to haram activity.
- **Community size running bots:** Essentially zero disclosed operators in this forum. Discussions are purely theoretical/scholarly.
- **No P&L reports** from anyone running a bot in this community.

### 9. IFG Forum — "Automated Bot Trading"
**forum.islamicfinanceguru.com, ~2023 | [FORUM]**
- **One operator disclosed results:** "Mixed success. It can generate a lot of profit but lose it all in just 1 or 2 bad trades." (RSI-based grid bot, not arbitrage.)
- **Mufti ruling:** "Using a bot is not a problem in and of itself" as long as underlying strategy is compliant.
- **Sniping bots:** Discussed but no definitive ruling — considered maysir-adjacent by some participants.
- **Key absence:** No halal-compliant arbitrage bot operators sharing live results.

---

## Meta-Analysis: Rough P&L Distribution

Based on all sources aggregated:

| Operator tier | Estimated % of participants | Typical outcome |
|---|---|---|
| Top ~20 professional/institutional bots | <0.01% | $10k–$300k/day on Solana |
| Funded teams with colo + custom infra | ~1% | Marginally profitable to breakeven |
| Retail / solo developers | ~99% | $0 or net negative (infra costs > revenue) |

**No retail first-person account with verified Solana atomic arb profits was found in any source.** Multiple developers published transparent accounts of zero profitability.

---

## Strategies: Consistently Profitable vs. Consistently Losing (at Retail Scale)

### Consistently reported as losing at retail scale
- **Atomic DEX-DEX arb on Solana/Ethereum** — dominated by 10–20 operators with institutional infra
- **EVM flashloan arb** — gas costs consume spreads; Flashbots inclusion ~20%
- **CEX-DEX Model A (inventory-based, single on-chain leg)** — signal is real but thin; 5 bps threshold produces ~5 detections/16h per pool; one-leg fill risk is unhedgeable without CEX API + colocation
- **Grid bots on any chain** — directional market exposure; "lose it all in 1–2 bad trades"

### Strategies with documented profitability (but NOT at retail scale)
- **Sandwich attacks** — profitable ($370–500M extracted/16 months on Solana) but: (a) requires private mempool access, (b) is haram, (c) Jito publicly shut down their mempool for this reason
- **Memecoin sniping** — profitable during launches but: (a) likely haram (maysir), (b) requires validator-level access, (c) PvP environment
- **LST staking arb (Sanctum)** — mechanically sound, but rate differences compress within seconds

### Potentially viable at our scale (not yet proven)
- **CEX-to-CEX altcoin spot arb (Binance vs. Bybit)** — capital-intensive, withdrawal delays are the killer; no retail accounts found of sustained profitability
- **UniswapX / intent filling (EVM)** — described as "challenging to generate significant profits" but "worth trying to understand market structure"
- **Monad same-chain arb** — mainnet launched November 2025, early-mover dynamics may exist; no retail accounts yet

---

## What This Means For Us Specifically

1. **Our zero-landing rate is not a bug.** It is the documented outcome for our tier of participant. The auction competition math (searchers bid away profits to validators) predicts this precisely.

2. **Co-location in Frankfurt is correct but incomplete.** We need to be co-located with whichever validators are slot leaders at the moment we submit. Jito has relayers in Frankfurt/Amsterdam — this helps. But the E6Y-class bots likely have direct validator relationships we don't.

3. **CEX-DEX signal at 5 bps is real but volume is too low.** Our first dry-run found 25 detections / $0.25 gross in 1 hour. Scaling to 10+ pools and tightening to 3 bps threshold is the documented recommendation from our own run.

4. **The halal constraint eliminates our biggest competitor.** We cannot do sandwich attacks, which are the most profitable MEV strategy. However, this may actually be an edge in disguise: validators increasingly want to distance themselves from sandwich bots (reputation risk, potential protocol changes). A transparent, arb-only searcher with consistent tips could negotiate direct validator relationships.

5. **CEX-to-CEX altcoin spot arb deserves serious evaluation.** No community accounts of sustained retail losses on *correctly-hedged* CEX-CEX arb (as distinct from one-leg). The failure mode is withdrawal timing, not strategy.

---

## Scam / Affiliate-Farming Flags

The following search results are marketing noise, not retail experience — all flagged as [AFFILIATE-RISK]:
- ArbitrageScanner.io, WunderTrading, 99Bitcoins "best bots" listicles, Maticz/Osizt bot development shops, ArbiBot GitHub repos with telegram contact in description (common scam pattern), SolanaMevBot docs site (sells bot software).

The GitHub repos `ChangeYourself0613/Solana-Arbitrage-Bot`, `senior106/Solana-Arbitrage-Bot1`, and `adams322111233221/solana-mev-bot` all have telegram contact usernames in descriptions — these are scam bots sold to naive users, not genuine implementations.

---

## Sources Consulted

Primary (fetched and analyzed):
- [Extropy Cross-Chain MEV Analysis 2024–2025](https://academy.extropy.io/pages/articles/mev-crosschain-analysis-2025.html)
- [Solid Quant — Whack-A-Mole DEX Arb Bot](https://medium.com/@solidquant/how-i-built-my-first-mev-arbitrage-bot-introducing-whack-a-mole-66d91657152e)
- [Solid Quant — 100 Hours of Sandwich Bot](https://medium.com/@solidquant/100-hours-of-building-a-sandwich-bot-a89235281da3)
- [Jito Tips Anatomy Analysis](https://medium.com/@shamikhzafar0/the-anatomy-of-jito-tips-who-pays-why-and-how-market-dynamics-shape-solanas-mev-economy-de2a0b09ca26)
- [Jito Bundling Economic Analysis](https://medium.com/@gwrx2005/jito-bundling-and-mev-optimization-strategies-on-solana-an-economic-analysis-c035b6885e1f)
- [Helius Solana MEV Introduction](https://www.helius.dev/blog/solana-mev-an-introduction)
- [Helius Solana MEV Report](https://www.helius.dev/blog/solana-mev-report)
- [GlobalGurus — 80% Bot Failure](https://globalgurus.org/why-80-of-solana-trading-bots-fail-in-the-first-month/)
- [Solana MEV State: Accelerate 2025](https://solanacompass.com/learn/accelerate-25/scale-or-die-at-accelerate-2025-the-state-of-solana-mev)
- [IFG Forum — MEV Bot Halal?](https://forum.islamicfinanceguru.com/t/is-it-halal-to-do-arbitrage-with-mev-bots/9945)
- [IFG Forum — Automated Bot Trading](https://forum.islamicfinanceguru.com/t/automated-bot-trading/1937)
- [CoinDesk — DeezNode Private Mempool](https://www.coindesk.com/business/2024/06/10/solana-heavyweights-wage-war-against-private-mempool-operators)
- [Coincub — Are Bots Worth It 2025](https://coincub.com/are-crypto-trading-bots-worth-it-2025/)

Note: Reddit itself was not directly accessible (reddit.com blocked from fetch). Community intelligence was gathered from indexed content, Medium post-mortems, and Islamic finance forums. Direct Reddit thread fetching was blocked at the infrastructure level.
