//! Tests for bundle confirmation tracker metrics integration.
//!
//! The core confirmation logic tests (check_bundle_statuses, tracker lifecycle)
//! are in src/executor/confirmation.rs #[cfg(test)] module, using mockito.
//! These tests verify the counter helper functions are callable.

#[test]
fn test_confirmation_counter_helpers_callable() {
    // These must not panic even without a metrics recorder installed.
    solana_mev_bot::metrics::counters::inc_bundles_confirmed();
    solana_mev_bot::metrics::counters::inc_bundles_dropped();
    solana_mev_bot::metrics::counters::add_confirmed_profit_lamports(100_000);
    solana_mev_bot::metrics::counters::add_confirmed_tips_paid_lamports(15_000);
    solana_mev_bot::metrics::counters::add_estimated_profit_lamports(100_000);
    solana_mev_bot::metrics::counters::add_estimated_tips_lamports(15_000);
}
