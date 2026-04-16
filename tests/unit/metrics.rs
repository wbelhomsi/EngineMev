#[test]
fn test_counter_helpers_callable_without_init() {
    // metrics crate handles calls before a recorder is installed (no-op).
    // These must not panic.
    solana_mev_bot::metrics::counters::inc_geyser_updates("orca");
    solana_mev_bot::metrics::counters::inc_geyser_parse_errors("raydium_amm");
    solana_mev_bot::metrics::counters::inc_routes_found(2);
    solana_mev_bot::metrics::counters::inc_opportunities("orca");
    solana_mev_bot::metrics::counters::inc_bundles_submitted();
    solana_mev_bot::metrics::counters::inc_bundles_skipped("dedup");
    solana_mev_bot::metrics::counters::inc_relay_submission("jito", "accepted");
    solana_mev_bot::metrics::counters::inc_bundle_build_errors();
    solana_mev_bot::metrics::counters::add_estimated_profit_lamports(50000);
    solana_mev_bot::metrics::counters::add_estimated_tips_lamports(25000);
    solana_mev_bot::metrics::counters::add_confirmed_profit_lamports(40000);
    solana_mev_bot::metrics::counters::add_confirmed_tips_paid_lamports(20000);
    solana_mev_bot::metrics::counters::inc_bundles_confirmed();
    solana_mev_bot::metrics::counters::inc_bundles_dropped();
    solana_mev_bot::metrics::counters::inc_vault_fetches("raydium_cp");
    solana_mev_bot::metrics::counters::set_cache_pools_tracked(1500);
    solana_mev_bot::metrics::counters::set_geyser_lag_slots(2);
    solana_mev_bot::metrics::counters::set_channel_backpressure(42);
    solana_mev_bot::metrics::counters::set_blockhash_age_ms(1200);
    solana_mev_bot::metrics::counters::record_route_calc_duration_us(350);
    solana_mev_bot::metrics::counters::record_simulation_duration_us(120);
    solana_mev_bot::metrics::counters::record_relay_latency_us("jito", 5400);
    solana_mev_bot::metrics::counters::record_geyser_parse_duration_us("orca", 80);
    // New error + profiling counters
    solana_mev_bot::metrics::counters::inc_simulation_rejected("unprofitable");
    solana_mev_bot::metrics::counters::inc_simulation_errors();
    solana_mev_bot::metrics::counters::inc_vault_fetch_errors("raydium_amm");
    solana_mev_bot::metrics::counters::inc_relay_errors("jito", "network_error");
    solana_mev_bot::metrics::counters::record_bundle_build_duration_us(450);
    solana_mev_bot::metrics::counters::record_pipeline_duration_us(2500);
}

#[test]
fn test_metrics_init_no_config_is_noop() {
    solana_mev_bot::metrics::init(None, None, "test");
}
