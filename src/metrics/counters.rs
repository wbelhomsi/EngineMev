//! Typed helper functions for all Prometheus metrics.
//!
//! Each function wraps a `metrics` crate macro call. The metrics crate uses
//! a global atomic recorder — no Arc, no locks, ~1-2ns per call.
//! Safe to call before init (no-op) and from any thread.

use metrics::{counter, gauge, histogram};

// ── Counters ──────────────────────────────────────────────────────────────

pub fn inc_geyser_updates(dex_type: &str) {
    counter!("geyser_updates_total", "dex_type" => dex_type.to_string()).increment(1);
}

pub fn inc_geyser_parse_errors(dex_type: &str) {
    counter!("geyser_parse_errors_total", "dex_type" => dex_type.to_string()).increment(1);
}

pub fn inc_geyser_reconnections() {
    counter!("geyser_reconnections_total").increment(1);
}

pub fn inc_routes_found(hops: usize) {
    counter!("routes_found_total", "hops" => hops.to_string()).increment(1);
}

pub fn inc_opportunities(dex_type: &str) {
    counter!("opportunities_total", "dex_type" => dex_type.to_string()).increment(1);
}

pub fn inc_bundles_submitted() {
    counter!("bundles_submitted_total").increment(1);
}

pub fn inc_bundles_skipped(reason: &str) {
    counter!("bundles_skipped_total", "reason" => reason.to_string()).increment(1);
}

pub fn inc_relay_submission(relay: &str, status: &str) {
    counter!("relay_submissions_total", "relay" => relay.to_string(), "status" => status.to_string()).increment(1);
}

pub fn inc_bundle_build_errors() {
    counter!("bundle_build_errors_total").increment(1);
}

/// Estimated profit at submission time (before on-chain confirmation).
pub fn add_estimated_profit_lamports(lamports: u64) {
    counter!("estimated_profit_lamports_total").increment(lamports);
}

/// Estimated tips at submission time (before on-chain confirmation).
pub fn add_estimated_tips_lamports(lamports: u64) {
    counter!("estimated_tips_lamports_total").increment(lamports);
}

/// Estimated profit for bundles that landed on-chain (per getBundleStatuses).
/// NOTE: This is the simulator's pre-submission estimate, NOT actual on-chain profit.
/// Actual profit may differ due to slippage or partial fills.
pub fn add_confirmed_profit_lamports(lamports: u64) {
    counter!("landed_estimated_profit_lamports_total").increment(lamports);
}

/// Estimated tips for bundles that landed on-chain.
pub fn add_confirmed_tips_paid_lamports(lamports: u64) {
    counter!("landed_estimated_tips_lamports_total").increment(lamports);
}

/// Bundles confirmed on-chain via getBundleStatuses.
pub fn inc_bundles_confirmed() {
    counter!("bundles_landed_total").increment(1);
}

/// Bundles submitted but never confirmed on-chain within timeout.
pub fn inc_bundles_dropped() {
    counter!("bundles_dropped_total").increment(1);
}

pub fn inc_vault_fetches(dex_type: &str) {
    counter!("vault_fetches_total", "dex_type" => dex_type.to_string()).increment(1);
}

pub fn inc_simulation_rejected(reason: &str) {
    counter!("simulation_rejected_total", "reason" => reason.to_string()).increment(1);
}

pub fn inc_simulation_errors() {
    counter!("simulation_rpc_errors_total").increment(1);
}

pub fn inc_vault_fetch_errors(dex_type: &str) {
    counter!("vault_fetch_errors_total", "dex_type" => dex_type.to_string()).increment(1);
}

pub fn inc_relay_errors(relay: &str, error_type: &str) {
    counter!("relay_errors_total", "relay" => relay.to_string(), "error_type" => error_type.to_string()).increment(1);
}

// ── Gauges ────────────────────────────────────────────────────────────────

pub fn set_cache_pools_tracked(count: usize) {
    gauge!("cache_pools_tracked").set(count as f64);
}

pub fn set_geyser_lag_slots(lag: u64) {
    gauge!("geyser_lag_slots").set(lag as f64);
}

pub fn set_channel_backpressure(len: usize) {
    gauge!("channel_backpressure").set(len as f64);
}

pub fn set_blockhash_age_ms(age_ms: u64) {
    gauge!("blockhash_age_ms").set(age_ms as f64);
}

// ── CEX-DEX P&L ──────────────────────────────────────────────────────────
//
// Exposed for the cexdex binary. The main engine ignores these.
//
// realized: cumulative arb profit (sum of net_profit_usd per dispatched bundle,
//           monotonically non-decreasing).
// unrealized: mark-to-market inventory drift = current_value - initial_value - realized.
// inventory_value: current MTM value of SOL+USDC balance.
// initial_inventory_value: snapshot at startup once SOL price is known.

pub fn set_cexdex_realized_pnl_usd(usd: f64) {
    gauge!("cexdex_realized_pnl_usd").set(usd);
}

pub fn set_cexdex_unrealized_pnl_usd(usd: f64) {
    gauge!("cexdex_unrealized_pnl_usd").set(usd);
}

pub fn set_cexdex_inventory_value_usd(usd: f64) {
    gauge!("cexdex_inventory_value_usd").set(usd);
}

pub fn set_cexdex_initial_inventory_value_usd(usd: f64) {
    gauge!("cexdex_initial_inventory_value_usd").set(usd);
}

pub fn set_cexdex_inventory_ratio(ratio: f64) {
    gauge!("cexdex_inventory_ratio").set(ratio);
}

pub fn set_cexdex_sol_price_usd(usd: f64) {
    gauge!("cexdex_sol_price_usd").set(usd);
}

// ── Histograms ────────────────────────────────────────────────────────────

pub fn record_route_calc_duration_us(us: u64) {
    histogram!("route_calc_duration_us").record(us as f64);
}

pub fn record_simulation_duration_us(us: u64) {
    histogram!("simulation_duration_us").record(us as f64);
}

pub fn record_relay_latency_us(relay: &str, us: u64) {
    histogram!("relay_latency_us", "relay" => relay.to_string()).record(us as f64);
}

pub fn record_geyser_parse_duration_us(dex_type: &str, us: u64) {
    histogram!("geyser_parse_duration_us", "dex_type" => dex_type.to_string()).record(us as f64);
}

pub fn record_bundle_build_duration_us(us: u64) {
    histogram!("bundle_build_duration_us").record(us as f64);
}

pub fn record_pipeline_duration_us(us: u64) {
    histogram!("pipeline_duration_us").record(us as f64);
}
