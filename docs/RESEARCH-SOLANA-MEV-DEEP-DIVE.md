# Solana MEV Deep Dive — Research Compilation

**Date:** 2026-04-04
**Sources:** 10 parallel research agents, 200+ sources analyzed

---

## 1. Open-Source Solana Arb Bots — Landscape Analysis

### Legitimate Projects (5 real out of ~50 examined)

**WARNING:** 90%+ of "Solana arb bot" repos on GitHub are scams, SEO spam, or plagiarized clones. The field is tiny.

| Repo | Stars | Lang | Architecture | DEXes | Key Insight |
|------|-------|------|-------------|-------|-------------|
| **0xNineteen/solana-arbitrage-bot** | 797 | Rust+TS | Custom on-chain program + off-chain client | 5 (legacy) | Gold standard: on-chain program for atomic multi-hop. Brute-force amount grid search. |
| **ChainBuff/sol-arb-bot** | 400 | TS | Self-hosted Jupiter node + Jito bundles | All Jupiter | Insight: self-hosted Jupiter with `--allow-circular-arbitrage`. ~85ms cycle time. |
| **Cetipoo/solana-onchain-arbitrage-bot** | 250 | Rust | On-chain program + 14 DEXes | 14 (most comprehensive) | Flashloan integration, ALTs, optimal trade size computation. Most similar to EngineMev. |
| **hodlwarden/solana-arbitrage-bot** | 157 | Rust | Jupiter polling + Yellowstone Geyser trigger | Jupiter | 9+ relays. Durable nonce accounts. Dual discovery (poll + Geyser). |
| **ApexArb/Solana-Arbitrage-Trading-Bot** | 58 | JS | Jupiter API polling + Jito | Jupiter | Simple but clean DRY_RUN mode and PnL calculation. |

### Confirmed Scams (Avoid)

| Repo | Red Flag |
|------|----------|
| x89/Solana-Arbitrage-Bot (1150★) | SEO spam, Telegram link, inflated stars |
| radioman/Auto-solana-trading-bot (1071★) | SEO spam 13x repeated keywords |
| **AV1080p/Solana-Arbitrage-Bot** (491★) | Generic repackage |
| solanmevbot/solana-mev-bot (213★) | Description repeats "solana mev bot" 23 times |
| SaoXuan/rust-mev-bot-shared (448★) | Closed-source binary, 10% revenue cut |

### Key Architectural Comparison

| Feature | EngineMev | 0xNineteen | Cetipoo | hodlwarden |
|---------|-----------|------------|---------|------------|
| Discovery | Geyser (unique!) | RPC polling | RPC polling | Jupiter + Geyser |
| Route finding | Custom O(1) index | Brute-force grid | Mint-grouped | Jupiter quotes |
| On-chain program | No (off-chain IX) | Yes (Anchor) | Yes (deployed) | No |
| Multi-relay | 5 relays | None shown | Multi-RPC spam | 9+ relays |
| Profit guarantee | Off-chain simulator | On-chain atomic | On-chain | Off-chain PnL |

### Critical Takeaway

**EngineMev's Geyser-first approach is unique** — no other open-source bot uses Geyser as primary discovery. This gives sub-50ms state awareness vs 200-500ms for Jupiter-based bots. But EngineMev lacks the **on-chain program** that 0xNineteen and Cetipoo use for atomic multi-hop swaps with dynamic output chaining.

---

## 2. Custom On-Chain Programs — The Production Standard

### Why a Custom Program Matters

Current EngineMev approach (off-chain IX building) has a fundamental flaw: hop 2's `amount_in` is hardcoded from hop 1's `estimated_output`. If hop 1 returns fewer tokens than expected, hop 2 fails or leaks value.

**With a custom program:**
```
instruction handler(ctx, input_amount, min_profit):
    start_balance = token_account.amount
    cpi::dex_a::swap(amount_in=input_amount, min_out=0)
    ctx.accounts.intermediate_token.reload()  // re-read ACTUAL output
    hop1_output = intermediate_token.amount    // REAL amount, not estimate
    cpi::dex_b::swap(amount_in=hop1_output, min_out=0)
    ctx.accounts.output_token.reload()
    require!(output_token.amount >= start_balance + min_profit)
```

### Preflight Error Filter Pattern (buffalojoec/arb-program)

The program is designed to ERROR when no arb exists. The client spams `simulateTransaction`. When simulation succeeds, a real arb exists → submit bundle. This eliminates wasted on-chain fees.

### CPI Compute Budget

| Route | Estimated CU | Fits in 400K? |
|-------|-------------|---------------|
| 2-hop (AMM + AMM) | ~75K + overhead | Yes |
| 2-hop (CLMM + Whirlpool) | ~130-180K + overhead | Yes (tight) |
| 3-hop (mixed) | ~200-280K | Likely needs more |

### Official CPI Crates

| Crate | For | Compatible with solana-sdk 2.2? |
|-------|-----|-------------------------------|
| `orca_whirlpools_client` | Orca swap CPI | Yes (^2.0) |
| `raydium_clmm` | Raydium CLMM CPI | Verify |
| `meteora-dlmm-sdk` | DLMM swap CPI | Verify |
| `jupiter-cpi` | Jupiter aggregator CPI | Yes (0.29+) |
| `phoenix-sdk-core` | Phoenix CPI | **NO** (requires 1.18.x) |

---

## 3. Jupiter Integration — When to Use It

### NOT for the Hot Path

Self-hosted Jupiter quote latency: 30-100ms. EngineMev's in-process math: microseconds. **100-1000x slower** — disqualifies Jupiter for competitive route discovery.

### YES for Validation/Fallback

- Validate profit estimates off hot path (in parallel)
- Build IXs for DEXes without custom builders (Phoenix, Manifest)
- Multi-tick CLMM simulation (Jupiter handles tick crossing correctly)

### Jupiter CPI is Now Viable

Post-January 2025, the "Loosen CPI size restriction" feature gate is deployed. CPI into Jupiter's `sharedAccountsRoute` works for most routes. But this is for on-chain programs, not off-chain TX builders like EngineMev.

---

## 4. Speed Optimization Techniques

### Latency Hierarchy (fastest to slowest)

| Optimization | Latency Saved | Effort |
|-------------|---------------|--------|
| **Co-locate bare-metal in Frankfurt** | 20-100ms on state reads | Infra ($50-200/mo) |
| **Pre-build instruction templates** | 1-5ms per opportunity | Medium code |
| **V0 transactions with ALTs** | Enables 3-hop, -30-50% tx size | Medium code |
| **Dynamic Jito tips via tip floor API** | Better landing rate | Low code |
| **Staked RPC for SWQoS** | 83% first-block hit rate | High infra |
| **Custom on-chain swap program** | 10-20% CU savings, mid-route abort | High dev |
| **ShredStream** | 50-200ms earlier than Geyser | Own node required |

### Stake-Weighted QoS (SWQoS)

Solana reserves **80% of QUIC connections for staked nodes**. Without stake, you compete for the 20% unstaked pool. With SWQoS, searchers achieve **83% first-block hit rates**.

### ShredStream vs Yellowstone gRPC

ShredStream delivers raw shreds 50-200ms before gossip propagation. EngineMev uses Yellowstone gRPC (second fastest). To get ShredStream, you need your own validator/RPC node + Jito proxy.

---

## 5. Address Lookup Tables (ALTs)

### Size Savings

| Component | Legacy TX | V0 + ALT | Savings |
|-----------|-----------|----------|---------|
| 30 accounts | 960 bytes | 219 bytes | **741 bytes (77%)** |
| Total TX | ~1159 bytes | ~418 bytes | Enables 3-hop |

### Implementation

```rust
// 1. Create ALT once at startup (~0.008 SOL rent, recoverable)
let (create_ix, alt_address) = create_lookup_table(authority, payer, recent_slot);

// 2. Extend with common addresses (DEX programs, token programs, tip accounts)
let extend_ix = extend_lookup_table(alt_address, authority, Some(payer), addresses);

// 3. Use in V0 transactions (on hot path)
let v0_message = v0::Message::try_compile(
    &signer.pubkey(), &instructions, &[alt_account], recent_blockhash,
)?;
let versioned_tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_message), &[signer])?;
```

**Jito restriction:** Tip accounts MUST stay in the static (non-ALT) portion. All other accounts can use ALT indices.

---

## 6. Jito Bundle Strategies

### Auction Dynamics

- 200ms tick intervals
- Evaluates **tip per compute unit**, not raw tip
- Packs maximum total tip value into block
- Account-level locking: conflicting bundles → highest bidder wins

### Bundle Rejection vs Non-Landing

| Rejected (simulation) | Accepted but not landing |
|----------------------|------------------------|
| Transaction error (wrong accounts) | Outbid by competitor |
| Stale state (arb expired) | Non-Jito leader (5% of slots) |
| Insufficient tip | Slot timing (arrived too late) |
| TX too large (>1232 bytes) | Account contention |

### Relay Market Share (Q3 2025 - Q1 2026)

| Relay | Share | Notes |
|-------|-------|-------|
| Jito (classic) | 61.5% | Dominant but declining |
| Jito BAM | 27.4% stake | New TEE-based, growing |
| ZeroSlot | 21% | Surged in 2025 |
| bloXroute | ~20% | $250M cumulative revenue |
| Nozomi (Temporal) | 7.5% peak | Single-TX, NOT bundle service |
| Astralane | Small | Tip refunds, revert protection |

**Nozomi finding:** Nozomi is a single-transaction landing service, NOT a bundle system. Our relay implementation may need adjustment.

---

## 7. Profitable Strategies (2025-2026)

### Ranked by Suitability for EngineMev

| Strategy | Profitability | Halal? | Recommended? |
|----------|-------------|--------|-------------|
| **CEX-DEX arb** (Binance→DEX) | HIGH | ✅ Yes | **#1 Priority** — different latency competition |
| **Cross-DEX arb** (Orca↔Raydium) | MODERATE | ✅ Yes | Already implemented, need speed |
| **LST rate arb** (Sanctum) | LOW-MODERATE | ✅ Yes | Keep enabled, thin margins |
| **JIT liquidity** | HIGH theoretical | ✅ Yes | Future — needs new architecture |
| **Intent backrunning** | MODERATE | ✅ Yes | Already captured by Geyser |
| Liquidation arb | HIGH | ❌ Haram | **FORBIDDEN** — debt exploitation |
| Token sniping | HIGH | ❌ Haram | **FORBIDDEN** — maysir (gambling) |
| Sandwich attacks | VERY HIGH | ❌ Haram | **FORBIDDEN** — exploitation |

### CEX-DEX is the Best Bet

EngineMev's 300ms latency is too slow for pure on-chain arb (co-located searchers win at <50ms). But CEX-DEX arb has a different competition profile — the signal comes from off-chain (Binance WebSocket), and fewer searchers have both CEX feeds and on-chain infrastructure. The existing pipeline (Geyser, router, simulator, bundle builder, relay) is fully reusable. Only one new component needed: Binance WebSocket feed.

**Strategy doc already exists:** `docs/STRATEGY-CEX-DEX-ARB.md`

### Market Intelligence

- $720M MEV revenue on Solana in 2025
- 90.4M successful arb transactions, avg profit $1.58
- Searchers pay 50-60% of profit in Jito tips
- One bot captures 42% of sandwich volume
- 95%+ of stake runs Jito client — MEV is the norm

---

## 8. Useful Rust Crates

### High Priority (unblocks roadmap)

| Crate | Purpose | Unblocks |
|-------|---------|----------|
| `ethnum` 1.5 | U256 arithmetic | CLMM multi-tick crossing |
| `orca_whirlpools_core` 2.0 | Tick math, swap quotes | Accurate CLMM quoting |
| `ahash` 0.8 | Fast HashMap for Pubkeys | Hot path performance |

### Medium Priority

| Crate | Purpose |
|-------|---------|
| `raydium_clmm` 0.1 | Typed CLMM accounts |
| `carbon-meteora-dlmm-decoder` 0.11 | DLMM bin array parsing |
| `orca_whirlpools_client` 2.0 | Real Orca swap IX builder |

### Already Have / No Change Needed

- `yellowstone-grpc-client` 12.1 — correct Geyser client
- `dashmap` 6 — already uses ahash internally
- `bytemuck` — extend to all parsers (zero-copy)
- `solana-sdk` 2.2 — ALT support built in

---

## 9. Community Resources

### Essential

| Resource | URL |
|----------|-----|
| Jito Docs | `docs.jito.wtf` |
| Jito Discord | `discord.gg/jito` |
| Jito Explorer | `explorer.jito.wtf` |
| Helius Blog | `helius.dev/blog` |
| Solana StackExchange | `solana.stackexchange.com` |

### Twitter/X to Follow

@jaborjito, @buffalu__, @0xMert_, @heliuslabs, @aeyakovenko, @nozomi_solana, @bloXroute, @meteoraag, @PhoenixTrade

---

## 10. Priority Roadmap for EngineMev

Based on all research, the highest-impact next steps:

1. **CEX-DEX arb** — Add Binance WebSocket feed, implement inventory-based model
2. **Custom on-chain program** — Atomic multi-hop with dynamic output chaining
3. **Address Lookup Tables** — V0 transactions, 77% account size reduction
4. **Co-location** — Frankfurt bare-metal ($50-200/mo)
5. **`ethnum` + `orca_whirlpools_core`** — Fix CLMM multi-tick math
6. **Dynamic Jito tips** — Tip floor API tracking
7. **Verify Nozomi API** — May not support bundles (single-TX only)
