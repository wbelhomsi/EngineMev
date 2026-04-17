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

/// Total detector hits (routes returned from check_all()).
pub fn inc_cexdex_opportunities() {
    counter!("cexdex_opportunities_total").increment(1);
}

/// Detector-level skip diagnostic. Labels why check_all() returned None at
/// each decision point. High-cardinality-safe — labels are a fixed set of
/// reason strings. Use to answer "why no opportunities?" without log diving.
pub fn inc_cexdex_detector_skip(reason: &'static str) {
    counter!("cexdex_detector_skip_total", "reason" => reason).increment(1);
}

/// Total simulator rejections (grouped by reason label).
pub fn inc_cexdex_sim_rejected(reason: &str) {
    counter!("cexdex_sim_rejected_total", "reason" => reason.to_string()).increment(1);
}

/// Every bundle we handed off to the relay dispatcher. NOT a land count.
pub fn inc_cexdex_bundles_attempted(relay: &str) {
    counter!("cexdex_bundles_attempted_total", "relay" => relay.to_string()).increment(1);
}

/// Bundle for this relay confirmed on-chain. Should fire at most once per
/// opportunity (only the relay whose tx landed — the others' txs fail the
/// nonce check and never count as confirmed).
pub fn inc_cexdex_bundles_confirmed(relay: &str) {
    counter!("cexdex_bundles_confirmed_total", "relay" => relay.to_string()).increment(1);
}

/// Bundle for this relay never landed (timed out or rejected).
pub fn inc_cexdex_bundles_dropped(relay: &str) {
    counter!("cexdex_bundles_dropped_total", "relay" => relay.to_string()).increment(1);
}

/// Cumulative USD paid as tips to this relay for confirmed bundles.
/// Paired with `cexdex_bundles_confirmed_total{relay}` gives cost-per-landing.
pub fn add_cexdex_tip_paid_usd(relay: &str, usd: f64) {
    if usd > 0.0 {
        counter!("cexdex_tip_paid_usd_micros_total", "relay" => relay.to_string())
            .increment((usd * 1_000_000.0) as u64);
    }
}

/// Sum of simulator net_profit_usd for every dispatched bundle. "Money we'd
/// have earned if everything landed" — gap vs cexdex_realized_pnl_usd = loss
/// to rate-limits, auction losses, and CEX drift.
pub fn add_cexdex_attempted_profit_usd(usd: f64) {
    // metrics crate counters accept u64; scale usd * 1_000_000 for 6-decimal precision.
    if usd > 0.0 {
        counter!("cexdex_attempted_profit_usd_micros_total").increment((usd * 1_000_000.0) as u64);
    }
}

/// Fires when checkout returned an in-flight nonce — signal we need
/// more nonce accounts.
pub fn inc_cexdex_nonce_collision_total() {
    counter!("cexdex_nonce_collision_total").increment(1);
}

/// Gauge (0..N) of the number of nonces currently in-flight.
pub fn set_cexdex_nonce_in_flight(count: usize) {
    gauge!("cexdex_nonce_in_flight").set(count as f64);
}

/// Increments on every Geyser-driven nonce state update. Used to
/// sanity-check that Geyser is keeping the cache current.
pub fn inc_cexdex_nonce_hash_refresh_total() {
    counter!("cexdex_nonce_hash_refresh_total").increment(1);
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
