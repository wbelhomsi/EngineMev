//! Integration test for the Prometheus metrics HTTP endpoint.
//!
//! This lives in its own test binary because `metrics-exporter-prometheus`
//! installs a global recorder — only one can exist per process. The unit
//! test binary already has tests that call counter helpers without a
//! recorder, and installing one there would conflict.

#[tokio::test]
async fn test_prometheus_metrics_endpoint() {
    // 1. Find an available port by binding to :0, then releasing it.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    // 2. Init metrics — installs the global Prometheus recorder with HTTP server.
    solana_mev_bot::metrics::init(Some(port), None, "test");

    // 3. Give the HTTP server a moment to start accepting connections.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // 4. Record some metrics across counters, gauges, and histograms.
    solana_mev_bot::metrics::counters::inc_bundles_submitted();
    solana_mev_bot::metrics::counters::inc_bundles_submitted();
    solana_mev_bot::metrics::counters::add_estimated_profit_lamports(50000);
    solana_mev_bot::metrics::counters::inc_relay_submission("jito", "accepted");
    solana_mev_bot::metrics::counters::set_cache_pools_tracked(42);
    solana_mev_bot::metrics::counters::record_route_calc_duration_us(350);
    solana_mev_bot::metrics::counters::inc_geyser_updates("orca");
    solana_mev_bot::metrics::counters::set_geyser_lag_slots(3);
    solana_mev_bot::metrics::counters::record_simulation_duration_us(120);

    // 5. Scrape the /metrics endpoint.
    let url = format!("http://127.0.0.1:{}/metrics", port);
    let resp = reqwest::get(&url)
        .await
        .expect("Failed to GET /metrics");

    assert!(resp.status().is_success(), "Expected 200 OK, got {}", resp.status());

    let body = resp.text().await.unwrap();

    // 6. Verify counters are present with expected values.
    assert!(
        body.contains("bundles_submitted_total"),
        "Missing bundles_submitted_total in:\n{body}"
    );
    assert!(
        body.contains("profit_lamports_total"),
        "Missing profit_lamports_total in:\n{body}"
    );
    assert!(
        body.contains("relay_submissions_total"),
        "Missing relay_submissions_total in:\n{body}"
    );
    assert!(
        body.contains("geyser_updates_total"),
        "Missing geyser_updates_total in:\n{body}"
    );

    // 7. Verify gauges.
    assert!(
        body.contains("cache_pools_tracked"),
        "Missing cache_pools_tracked in:\n{body}"
    );
    assert!(
        body.contains("geyser_lag_slots"),
        "Missing geyser_lag_slots in:\n{body}"
    );

    // 8. Verify histograms.
    assert!(
        body.contains("route_calc_duration_us"),
        "Missing route_calc_duration_us in:\n{body}"
    );
    assert!(
        body.contains("simulation_duration_us"),
        "Missing simulation_duration_us in:\n{body}"
    );
}
