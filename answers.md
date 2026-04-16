# Token Program Offsets in Orca Whirlpool and Raydium CLMM Pool Accounts

## TL;DR

**Your guess is incorrect.** Neither account stores `token_program_a/b` or `token_program_0/1`. These fields **do not exist** in the Whirlpool or Raydium CLMM PoolState account layouts. You need a different approach: fetch each mint account and inspect its `owner` field (SPL Token vs Token-2022).

---

## 1. Orca Whirlpool (Whirlpool account, 653 bytes)

**No `token_program_a` / `token_program_b` fields.**

Verified against the struct parser at `src/modules/SVM/dexes/Whirlpool/parsers/parseWhirlpoolLPAccountData.ts:13-43`. The full layout:

| Offset | Size    | Field                      |
| ------ | ------- | -------------------------- |
| 0      | 8       | discriminator              |
| 8      | 32      | whirlpoolsConfig           |
| 40     | 1       | whirlpoolBump              |
| 41     | 2       | tickSpacing                |
| 43     | 2       | feeTierIndexSeed           |
| 45     | 2       | feeRate                    |
| 47     | 2       | protocolFeeRate            |
| 49     | 16      | liquidity (u128)           |
| 65     | 16      | sqrtPrice (u128)           |
| 81     | 4       | tickCurrentIndex (i32)     |
| 85     | 8       | protocolFeeOwedA           |
| 93     | 8       | protocolFeeOwedB           |
| 101    | 32      | **tokenMintA**             |
| 133    | 32      | tokenVaultA                |
| 165    | 16      | feeGrowthGlobalA           |
| 181    | 32      | **tokenMintB**             |
| 213    | 32      | tokenVaultB                |
| 245    | 16      | feeGrowthGlobalB           |
| 261    | 8       | rewardLastUpdatedTimestamp |
| 269    | 3 × 128 | rewardInfos[3]             |

Total: 269 + 384 = 653 bytes. No token-program fields anywhere. Your guessed offsets 589 / 621 fall inside `rewardInfos`, not token programs.

**How token programs are obtained in this codebase:** they are not read from the pool account. Whirlpool swap instructions receive `token_program_a` and `token_program_b` as separate instruction accounts; the codebase plugs in either `TOKEN_PROGRAM_ID` or `TOKEN_2022_PROGRAM_ID` from chain config / mint inspection. See `src/modules/SVM/dexes/Whirlpool/startup/Whirlpool.startup.ts` and the Whirlpool quoter.

---

## 2. Raydium CLMM PoolState (~1544 bytes)

**No `token_program_0` / `token_program_1` fields.**

Verified against `src/modules/SVM/dexes/RaydiumCLMM/parsers/parseRaydiumCLMMLPAccountData.ts:13-68`. Key offsets:

| Offset | Size | Field                       |
| ------ | ---- | --------------------------- |
| 0      | 8    | discriminator               |
| 8      | 1    | bump                        |
| 9      | 32   | ammConfig                   |
| 41     | 32   | owner                       |
| 73     | 32   | **tokenMint0**              |
| 105    | 32   | **tokenMint1**              |
| 137    | 32   | tokenVault0                 |
| 169    | 32   | tokenVault1                 |
| 201    | 32   | observationKey              |
| 233    | 1    | mintDecimals0               |
| 234    | 1    | mintDecimals1               |
| 235    | 2    | tickSpacing                 |
| 237    | 16   | liquidity                   |
| ...    | ...  | (rewards, bitmaps, padding) |

The trailing `padding1 = u64[24]` + `padding2 = u64[32]` makes the struct 1544 bytes. If you're seeing 1560, double-check your source — the on-chain anchor struct has historically been 1544. Regardless, no token-program pubkeys are stored.

**How token programs are obtained:** Raydium CLMM swap instructions take the two token programs as instruction accounts. The codebase fetches them from chain config / mint owner, not the PoolState.

---

## Recommended Approach (Rust)

Since the pool account doesn't contain the token programs, fetch each mint via `get_account` (or `get_multiple_accounts` to batch both) and read the `owner` field:

- `mint_account.owner == spl_token::ID` → SPL Token (legacy).
- `mint_account.owner == spl_token_2022::ID` → Token-2022.

Use that program ID when building the ATA creation instruction. Most Whirlpool/CLMM pools are still legacy SPL-Token on both sides, but Token-2022 pools do exist and must be handled via the mint owner.

---

# Part 2 — Answering the Arb-Bot Question

You're hitting 41% `IncorrectProgramId` on the _second_ ATA because you don't know each mint's token program in time. Here's exactly how this codebase avoids that problem, and what it implies for your bot.

## Key insight: don't look up mints — look up _vaults_

The trick this codebase uses is that you have to read the pool's vault accounts anyway (to get reserves). The vault's `owner` field **is** the token program for that side of the pool. One `getMultipleAccountsInfo([vaultA, vaultB])` call gives you reserves AND both token programs with no extra round-trip.

Core primitive: `src/modules/SVM/dexes/utils/getReservesInCacheForm.ts:33-71`

```ts
svmChain.connection.getMultipleAccountsInfo(reservesPublicKeys, {
  commitment: "processed",
  dataSlice: { offset: 64, length: 8 }, // skip mint+owner, read only u64 amount
});
// Then for each vault:
const tokenProgramId = associatedTokenAccountInfo.owner;
if (svmChain.getTokenProgramPublicKey().equals(tokenProgramId))
  return `Token:${amount}:${slot}`;
if (svmChain.getTokenProgram2022PublicKey().equals(tokenProgramId))
  return `Token2022:${amount}:${slot}`;
```

Note the `dataSlice` — only 8 bytes of payload per vault come back; the `owner` pubkey comes back for free in the `AccountInfo` header. So one batched RPC resolves both token programs for a pool.

That result is stored per-pool in Redis as `"Token|Token2022:amount:slot"` (the `ReserveInCacheForm` string in `types/`), and consumed at swap-time in the quoter (`Whirlpool/quoter/index.ts:139-197`, `RaydiumCLMM/quoter/index.ts:126-191`) where it drives the choice between `TOKEN_PROGRAM_ID` and `TOKEN_2022_PROGRAM_ID` for ATA derivation.

There is also `getTokenType()` (`src/modules/SVM/utils/mint.utils.ts:10-36`) which fetches a single mint's owner with 1-day memoization — but this is the **fallback** path, only used when no pool context is available. On the hot path, the vault owner is the source of truth.

## Answers to your five questions, as implemented here

**1. When/where are token programs resolved?**

- Lazily, _per pool_, at the moment the pool is first observed by the program-account listener.
- Not at startup. No `getProgramAccounts` scan of the two token programs.
- Not per-mint either. Per _vault_, and the vault owner is read as a side-effect of the reserve fetch.
- Source: `Whirlpool.startup.ts` and `RaydiumCLMM.startup.ts` wire `onProgramAccountChange` for both SPL Token and Token-2022 program IDs so the listener itself surfaces the vault-side token program.

**2. Do you block opportunity processing until the program is known?**

- Yes — if `lpData.tokenReserve` hasn't been resolved for a pool, the quoter doesn't quote it (the `.type` field wouldn't exist). But because the one `getMultipleAccountsInfo` that populates reserves _also_ populates token programs, there's no separate blocking window: it's the same call.
- The codebase treats "have reserves" and "have token programs" as one resolution step — there's no in-between state where you know one but not the other. This is what prevents "fast opportunities get sent with the wrong program".
- There's no preemptive/speculative dispatch that would let a pool be traded with unknown programs.

**3. Persistent cache across restarts?**

- Yes. Pool cache is in Redis/Valkey (`cache.data-access.ts`), keyed by `{token}:{lp}`, with the token-program encoded inside `tokenReserve: ReserveInCacheForm`. TTL is `LP_TTL_SECONDS`.
- The single-mint `getTokenType()` cache is in-process (memoizee, 1 day, not persisted).
- There is **no** separate `mint → program` table. It's always derived via the vault.

**4. Orca/Raydium-specific batching strategy?**

- Pool arrives via `onProgramAccountChange` → parser extracts `tokenVaultA/B` (Whirlpool offsets 133, 213) or `tokenVault0/1` (CLMM offsets 137, 169) — no RPC needed, these are in the pool account data.
- Single `getMultipleAccountsInfo([vaultA, vaultB])` with `dataSlice: { offset: 64, length: 8 }`.
- That one call returns reserves (via the 8-byte slice) and both token programs (via each `AccountInfo.owner`).
- ATAs can then be derived deterministically for both sides.

**5. Fastest path from "Geyser event for unknown pool" to "can submit arb"?**
The codebase's sequence is:

```
Geyser/WS event → parse pool account (offsets only, 0 RPC)
              → extract tokenMint{A,B}, tokenVault{A,B}
              → getMultipleAccountsInfo([vaultA, vaultB], {dataSlice:{offset:64,length:8}})
              → owner fields → token programs; data → reserves
              → derive ATAs with correct program → build swap IX
```

One RPC between "pool appears" and "ready to trade". That's the floor unless you pre-warm.

## What you're probably missing in your bot

The 41% `IncorrectProgramId` pattern strongly suggests one of:

1. **You're reading token programs from mint accounts instead of vault accounts.** Mints need a separate fetch; vaults you need anyway. Switch to: `getMultipleAccountsInfo([vaultA, vaultB])` and read `owner`.
2. **You're parsing pool → deriving ATA in the same tick without awaiting the vault fetch.** This codebase couples them into a single Promise chain; if your bot fires the ATA-create before the vault resolution resolves, you'll race.
3. **You have no per-vault owner cache and refetch every arb.** Persist `vault → token_program` in memory once resolved; Whirlpool/CLMM vaults don't change program during a pool's lifetime. This codebase caches the resolved value in Redis with the pool entry.

## Pre-warming options (not in this codebase, but viable for your latency profile)

Given 50 ms Geyser latency and sub-ms competition, consider:

- **Program-account scan at startup**: one-time `getProgramAccounts(TOKEN_2022_PROGRAM_ID)` filtering for `Mint` accounts (165-byte data). Build an in-memory `Set<mint>` of all Token-2022 mints. Any mint not in the set is legacy. This is a cold-start cost (minutes), but subsequent resolution is O(1) local lookup — zero RPC. Persist the set to disk; refresh periodically with slot-windowed re-scans.
- **Pair-program subscription**: subscribe to `onProgramAccountChange(TOKEN_2022_PROGRAM_ID)` filtered to `dataSize=165` (new mints only). Keeps your set live.
- **Vault-owner cache (simpler, narrower)**: on every pool subscription, populate a `vault → token_program` map. When a new pool for _the same mints_ shows up, you already know both programs. Bypasses the one-RPC step entirely.

The codebase here does none of that — it relies on the vault fetch being fast enough because it's batched and `dataSlice`-limited. For a sub-ms bot in Frankfurt, pre-warming Token-2022 mint set into a local Bloom filter / Set is likely the cleanest answer, because then you never need a separate RPC to classify a mint.

## File pointers

- Vault-owner-as-token-program trick: `src/modules/SVM/dexes/utils/getReservesInCacheForm.ts:14-31, 39-46`
- Standalone mint resolver (fallback): `src/modules/SVM/utils/mint.utils.ts:10-36`
- Whirlpool pool layout (so you can grab `tokenVaultA/B` at offsets 133 / 213): `src/modules/SVM/dexes/Whirlpool/parsers/parseWhirlpoolLPAccountData.ts`
- Raydium CLMM pool layout (`tokenVault0/1` at 137 / 169): `src/modules/SVM/dexes/RaydiumCLMM/parsers/parseRaydiumCLMMLPAccountData.ts`
- Dual-program listeners (where "unknown pool" events originate): `src/modules/SVM/dexes/Whirlpool/startup/Whirlpool.startup.ts`, `…/RaydiumCLMM/startup/RaydiumCLMM.startup.ts`
- Quoter consumption (how ATA program is chosen from the cached type): `Whirlpool/quoter/index.ts:139-197`, `RaydiumCLMM/quoter/index.ts:126-191`
