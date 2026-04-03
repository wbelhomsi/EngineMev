# Next Session TODO

## Status: Jito accepts bundles at 66ms. Need rate limiter + verify on-chain landing.

## 1. Add Jito rate limiter (CRITICAL)
Jito allows 1 bundle/sec unauthenticated. We're submitting every opportunity (~5/sec), triggering rate limit backoff after the first bundle. Only the first bundle per session gets accepted.

**Fix:** Add rate limiter back in `src/main.rs` router loop:
```rust
let mut last_submission = Instant::now() - Duration::from_secs(5);
const MIN_INTERVAL: Duration = Duration::from_millis(1500);

// Before submitting:
if last_submission.elapsed() < MIN_INTERVAL { continue; }
// After submitting:
last_submission = Instant::now();
```

## 2. Verify Astralane auth fix works
Changed from `api_key` header to `?api-key=` query param. Was getting 401 Unauthorized. Need to test if the query param approach works.

## 3. Check if Jito-accepted bundles land on-chain
Previous accepted bundle IDs:
- `659231248cc2a7f39adb8e6d8038191423d4316ef287db1520cfcdd3934681d8`
- `cf6ec769977e815666c46bcc0eb9ef337877e11e8e088a71a3c4f7064a6235b5`

Check: https://explorer.jito.wtf/bundle/<id>
If they show as "landed" — we're making money. If "dropped" — the arb was already captured by faster searchers.

## 4. Apply for Jito UUID (pending)
Application submitted via pastebin. Once approved, set `JITO_AUTH_UUID` in .env for higher rate limits.

## What's working
- Full pipeline: Geyser → 8 DEX parsers → route → simulate → build bundle → ATA creation → compute budget → tip → submit
- Mint program cache (SPL Token vs Token-2022) — correctly identifies pump.fun tokens
- Fire-and-forget mint fetch with router gating (race condition fixed)
- Jito Frankfurt at 66ms, Astralane FRA with revert_protect
- Compute budget IX (400K CU limit, 1000 micro-lamport priority)
- 66 tests passing
- Balance: 0.75 SOL (untouched — minimum_amount_out + revert_protect)

## Key findings from this session
- Jito rate limit: 1/sec unauth, "Network congested" after backoff
- Astralane was 401 (wrong auth method) — fixed to query param
- Mint cache race condition — fixed (fire-and-forget + gate router on cached mints)
- DLMM bitmap extension — pass program ID for Option<UncheckedAccount> None
- Token-2022 detection — getAccountInfo(mint).owner via RPC, cached in DashMap
