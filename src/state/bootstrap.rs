// Pool bootstrapping via getProgramAccounts has been replaced by lazy discovery.
// Pools are now discovered automatically when Geyser streams their first account update.
// Raydium AMM v4 and CP vault balances are fetched lazily per-pool via getMultipleAccounts.
//
// See docs/superpowers/specs/2026-04-02-geyser-pool-state-parsing-design.md for details.
