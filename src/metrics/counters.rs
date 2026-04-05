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

pub fn add_profit_lamports(lamports: u64) {
    counter!("profit_lamports_total").increment(lamports);
}

pub fn add_tips_paid_lamports(lamports: u64) {
    counter!("tips_paid_lamports_total").increment(lamports);
}

pub fn inc_vault_fetches(dex_type: &str) {
    counter!("vault_fetches_total", "dex_type" => dex_type.to_string()).increment(1);
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
