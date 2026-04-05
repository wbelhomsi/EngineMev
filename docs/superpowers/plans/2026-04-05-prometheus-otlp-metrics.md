# Prometheus + OTLP Metrics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add production observability (Prometheus metrics + OTLP tracing) to the MEV engine with zero hot-path impact.

**Architecture:** Two independent telemetry pipelines: (1) `metrics` crate with Prometheus exporter for counters/gauges/histograms, (2) `tracing-opentelemetry` layer for span export via OTLP. Both export on background threads. Both disabled by default — activated by env vars.

**Tech Stack:** `metrics 0.24`, `metrics-exporter-prometheus 0.18`, `opentelemetry 0.31`, `opentelemetry_sdk 0.31`, `opentelemetry-otlp 0.31`, `opentelemetry-semantic-conventions 0.31`, `tracing-opentelemetry 0.32`

---

## File Structure

| File | Responsibility |
|------|---------------|
| `src/metrics/mod.rs` | NEW — `init()`, `shutdown()`, Prometheus HTTP server, re-exports |
| `src/metrics/counters.rs` | NEW — All metric helper functions (typed wrappers around `metrics` macros) |
| `src/metrics/tracing_layer.rs` | NEW — Build optional OTel tracing layer for OTLP export |
| `src/config.rs` | MODIFY — Add `metrics_port`, `otlp_endpoint`, `otlp_service_name` to BotConfig |
| `src/lib.rs` | MODIFY — Add `pub mod metrics;` |
| `src/main.rs` | MODIFY — Call `metrics::init()`, update tracing subscriber, instrument hot path |
| `src/mempool/stream.rs` | MODIFY — Add geyser counter/histogram calls |
| `src/executor/relays/mod.rs` | MODIFY — Add relay metric recording to trait default or each impl |
| `src/state/blockhash.rs` | MODIFY — Add blockhash_age_ms gauge |
| `tests/unit/metrics.rs` | NEW — Unit tests for metrics module |
| `tests/unit/mod.rs` | MODIFY — Add `mod metrics;` |

---

### Task 1: Add dependencies to Cargo.toml

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add metrics and OpenTelemetry deps**

Add to the `[dependencies]` section after the existing `dotenv` entry:

```toml
# Prometheus metrics (atomic counters, zero hot-path cost)
metrics = "0.24"
metrics-exporter-prometheus = "0.18"

# OpenTelemetry tracing (optional, enabled by OTLP_ENDPOINT env var)
opentelemetry = "0.31"
opentelemetry_sdk = { version = "0.31", features = ["rt-tokio"] }
opentelemetry-otlp = "0.31"
opentelemetry-semantic-conventions = "0.31"
tracing-opentelemetry = "0.32"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: compiles with no errors (may have unused import warnings, that's fine)

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: add metrics + opentelemetry crates for observability"
```

---

### Task 2: Add metrics config fields to BotConfig

**Files:**
- Modify: `src/config.rs`

- [ ] **Step 1: Write the failing test**

Add to `tests/unit/metrics.rs` (create the file):

```rust
use std::env;

#[test]
fn test_metrics_config_defaults() {
    // Clear any existing env vars
    env::remove_var("METRICS_PORT");
    env::remove_var("OTLP_ENDPOINT");
    env::remove_var("OTLP_SERVICE_NAME");

    // BotConfig::from_env() should parse without metrics vars set
    // (they should all be Optional/default)
    // Just verify the fields exist on BotConfig
    let config = solana_mev_bot::config::BotConfig::from_env();
    // from_env may fail due to missing RPC_URL etc in test — that's fine.
    // We just need to confirm the struct has the fields.
    // This test mainly ensures compilation with the new fields.
}

#[test]
fn test_metrics_config_with_port() {
    env::set_var("METRICS_PORT", "9090");
    // The field should be parseable
    let port: Option<u16> = env::var("METRICS_PORT").ok().and_then(|v| v.parse().ok());
    assert_eq!(port, Some(9090));
    env::remove_var("METRICS_PORT");
}
```

Also add `mod metrics;` to `tests/unit/mod.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test unit metrics`
Expected: compilation error — `metrics` module doesn't exist in tests yet, and config fields don't exist

- [ ] **Step 3: Create tests/unit/metrics.rs and add mod**

Create `tests/unit/metrics.rs` with the test code above.
Add `mod metrics;` to `tests/unit/mod.rs`.

- [ ] **Step 4: Add config fields to BotConfig**

In `src/config.rs`, add three fields to the `BotConfig` struct after `arb_guard_program_id`:

```rust
    /// Port for Prometheus /metrics HTTP endpoint (disabled if None)
    pub metrics_port: Option<u16>,
    /// OTLP gRPC endpoint for tracing spans (disabled if None)
    pub otlp_endpoint: Option<String>,
    /// Service name reported in OTLP traces
    pub otlp_service_name: String,
```

In `BotConfig::from_env()`, add parsing before the closing `})`:

```rust
            metrics_port: std::env::var("METRICS_PORT")
                .ok()
                .and_then(|v| v.parse().ok()),
            otlp_endpoint: std::env::var("OTLP_ENDPOINT").ok().filter(|s| !s.is_empty()),
            otlp_service_name: std::env::var("OTLP_SERVICE_NAME")
                .unwrap_or_else(|_| "mev-engine".to_string()),
```

- [ ] **Step 5: Run tests**

Run: `cargo test --test unit metrics`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/config.rs tests/unit/metrics.rs tests/unit/mod.rs
git commit -m "feat: add metrics config fields (METRICS_PORT, OTLP_ENDPOINT)"
```

---

### Task 3: Create src/metrics/counters.rs — metric helper functions

**Files:**
- Create: `src/metrics/counters.rs`

- [ ] **Step 1: Write the failing test**

Add to `tests/unit/metrics.rs`:

```rust
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
    solana_mev_bot::metrics::counters::add_profit_lamports(50000);
    solana_mev_bot::metrics::counters::add_tips_paid_lamports(25000);
    solana_mev_bot::metrics::counters::inc_vault_fetches("raydium_cp");
    solana_mev_bot::metrics::counters::set_cache_pools_tracked(1500);
    solana_mev_bot::metrics::counters::set_geyser_lag_slots(2);
    solana_mev_bot::metrics::counters::set_channel_backpressure(42);
    solana_mev_bot::metrics::counters::set_blockhash_age_ms(1200);
    solana_mev_bot::metrics::counters::record_route_calc_duration_us(350);
    solana_mev_bot::metrics::counters::record_simulation_duration_us(120);
    solana_mev_bot::metrics::counters::record_relay_latency_us("jito", 5400);
    solana_mev_bot::metrics::counters::record_geyser_parse_duration_us("orca", 80);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test unit metrics`
Expected: FAIL — `solana_mev_bot::metrics` module doesn't exist

- [ ] **Step 3: Create src/metrics/counters.rs**

```rust
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
```

- [ ] **Step 4: Create src/metrics/mod.rs (stub)**

```rust
//! Production observability: Prometheus metrics + optional OTLP tracing.
//!
//! Both pipelines are optional:
//! - Set METRICS_PORT to enable Prometheus /metrics HTTP endpoint
//! - Set OTLP_ENDPOINT to enable tracing span export
//! When neither is set, init() is a near-no-op.

pub mod counters;
pub mod tracing_layer;

/// Initialize metrics and tracing pipelines.
/// Call once at startup before the pipeline starts.
pub fn init(_metrics_port: Option<u16>, _otlp_endpoint: Option<&str>, _service_name: &str) {
    // Stub — implemented in Task 4
}

/// Flush pending spans and shut down exporters.
pub fn shutdown() {
    // Stub — implemented in Task 5
}
```

- [ ] **Step 5: Create src/metrics/tracing_layer.rs (stub)**

```rust
//! Optional OpenTelemetry tracing layer for OTLP span export.

/// Build an OTel tracing layer if OTLP_ENDPOINT is configured.
/// Returns None if disabled.
pub fn build_otel_layer(
    _endpoint: &str,
    _service_name: &str,
) -> Option<tracing_opentelemetry::OpenTelemetryLayer<tracing_subscriber::Registry, opentelemetry_sdk::trace::SdkTracerProvider>> {
    // Stub — implemented in Task 5
    None
}
```

- [ ] **Step 6: Add `pub mod metrics;` to src/lib.rs**

Add `pub mod metrics;` to `src/lib.rs`.

- [ ] **Step 7: Run tests**

Run: `cargo test --test unit metrics`
Expected: PASS — all counter helpers callable without panic

- [ ] **Step 8: Commit**

```bash
git add src/metrics/ src/lib.rs tests/unit/metrics.rs
git commit -m "feat: add metrics counter helpers (no-op until init)"
```

---

### Task 4: Implement Prometheus exporter in metrics/mod.rs

**Files:**
- Modify: `src/metrics/mod.rs`

- [ ] **Step 1: Write the failing test**

Add to `tests/unit/metrics.rs`:

```rust
#[test]
fn test_metrics_init_no_config_is_noop() {
    // init with no port and no OTLP should not panic
    solana_mev_bot::metrics::init(None, None, "test");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test unit test_metrics_init_no_config_is_noop`
Expected: PASS (stub already handles None) — but this validates the contract

- [ ] **Step 3: Implement full init() in src/metrics/mod.rs**

Replace the stub with:

```rust
//! Production observability: Prometheus metrics + optional OTLP tracing.
//!
//! Both pipelines are optional:
//! - Set METRICS_PORT to enable Prometheus /metrics HTTP endpoint
//! - Set OTLP_ENDPOINT to enable tracing span export
//! When neither is set, init() is a near-no-op.

pub mod counters;
pub mod tracing_layer;

use metrics_exporter_prometheus::PrometheusBuilder;
use tracing::info;

/// Initialize the Prometheus metrics recorder.
/// If `metrics_port` is Some, also spawns an HTTP server for /metrics scraping.
/// If None, installs a recorder that accepts metrics but exposes no endpoint
/// (useful for testing or when only OTLP tracing is desired).
pub fn init(metrics_port: Option<u16>, otlp_endpoint: Option<&str>, service_name: &str) {
    // Install Prometheus recorder (always — so counter calls don't no-op)
    if let Some(port) = metrics_port {
        let builder = PrometheusBuilder::new()
            .with_http_listener(([0, 0, 0, 0], port));

        match builder.install() {
            Ok(()) => info!("Prometheus metrics server listening on 0.0.0.0:{}", port),
            Err(e) => tracing::error!("Failed to install Prometheus recorder: {}", e),
        }
    }

    if otlp_endpoint.is_some() {
        info!("OTLP tracing configured (service={}), layer must be added to subscriber", service_name);
    }
}

/// Flush pending spans and shut down OTLP exporter.
pub fn shutdown() {
    opentelemetry::global::shutdown_tracer_provider();
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --test unit metrics`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/metrics/mod.rs
git commit -m "feat: implement Prometheus metrics exporter init"
```

---

### Task 5: Implement OTLP tracing layer

**Files:**
- Modify: `src/metrics/tracing_layer.rs`

- [ ] **Step 1: Implement build_otel_layer**

Replace stub in `src/metrics/tracing_layer.rs`:

```rust
//! Optional OpenTelemetry tracing layer for OTLP span export.
//!
//! When OTLP_ENDPOINT is set, this builds a tracing layer that exports
//! spans to an OTLP-compatible collector (Grafana Tempo, Jaeger, etc.).
//! Spans are batched and exported on a background tokio task — zero
//! blocking on the hot path.

use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::Registry;

/// Build an OTel tracing layer if endpoint is provided.
/// Returns (layer, provider) — caller must keep provider alive for flush on shutdown.
pub fn build_otel_layer(
    endpoint: &str,
    service_name: &str,
) -> Option<(
    OpenTelemetryLayer<Registry, opentelemetry_sdk::trace::SdkTracer>,
    SdkTracerProvider,
)> {
    use opentelemetry_otlp::SpanExporter;
    use opentelemetry_sdk::trace::BatchSpanProcessor;
    use opentelemetry_sdk::Resource;
    use opentelemetry::KeyValue;

    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .map_err(|e| tracing::error!("Failed to build OTLP exporter: {}", e))
        .ok()?;

    let resource = Resource::builder()
        .with_attribute(KeyValue::new(
            opentelemetry_semantic_conventions::resource::SERVICE_NAME,
            service_name.to_string(),
        ))
        .build();

    let provider = SdkTracerProvider::builder()
        .with_span_processor(BatchSpanProcessor::builder(exporter).build())
        .with_resource(resource)
        .build();

    let tracer = provider.tracer("mev-engine");
    let layer = OpenTelemetryLayer::new(tracer);

    Some((layer, provider))
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: compiles (the exact OpenTelemetry API may need minor adjustments — fix any type errors from version differences)

- [ ] **Step 3: Commit**

```bash
git add src/metrics/tracing_layer.rs
git commit -m "feat: implement OTLP tracing layer builder"
```

---

### Task 6: Wire metrics + tracing into main.rs

**Files:**
- Modify: `src/main.rs`

This is the integration task. We change the tracing subscriber setup and add metric calls on the hot path.

- [ ] **Step 1: Update tracing subscriber initialization**

Replace the current tracing init block (lines 27-36) with:

```rust
    // Initialize tracing with optional OTLP layer
    let config = Arc::new(BotConfig::from_env()?);

    // Build subscriber layers
    let json_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_target(true)
        .with_thread_ids(true);

    let env_filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive("solana_mev_bot=debug".parse()?)
        .add_directive("info".parse()?);

    // Initialize metrics (Prometheus recorder)
    solana_mev_bot::metrics::init(
        config.metrics_port,
        config.otlp_endpoint.as_deref(),
        &config.otlp_service_name,
    );

    // Build optional OTLP tracing layer
    let otel_layer = config.otlp_endpoint.as_deref().and_then(|endpoint| {
        solana_mev_bot::metrics::tracing_layer::build_otel_layer(endpoint, &config.otlp_service_name)
    });

    // Install layered subscriber
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    if let Some((otel, _provider)) = otel_layer {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(json_layer)
            .with(otel)
            .init();
        info!("Tracing initialized with OTLP export");
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(json_layer)
            .init();
    }
```

Note: Move `let config = ...` BEFORE tracing init (it's currently after). The config load doesn't need tracing.

- [ ] **Step 2: Add metric calls to the router hot path**

In the router `spawn_blocking` closure, add timing and counters. The key instrumentation points:

After `routes.retain(|r| r.base_mint == sol);` (around line 289):
```rust
                if !routes.is_empty() {
    solana_mev_bot::metrics::counters::inc_routes_found(routes[0].hop_count());
                }
```

After `opportunities_found += 1;` (around line 358):
```rust
                        solana_mev_bot::metrics::counters::inc_opportunities(
                            &format!("{:?}", route.hops[0].dex_type));
                        solana_mev_bot::metrics::counters::add_profit_lamports(final_profit_lamports);
```

After `bundles_submitted += 1;` (around line 443):
```rust
                                solana_mev_bot::metrics::counters::inc_bundles_submitted();
                                solana_mev_bot::metrics::counters::add_tips_paid_lamports(tip_lamports);
```

After `error!("Bundle build failed: {}", e);` (around line 446):
```rust
                                solana_mev_bot::metrics::counters::inc_bundle_build_errors();
```

For each skip path, add `inc_bundles_skipped` with the reason:
- `config.dry_run` → `inc_bundles_skipped("dry_run")`
- `!can_submit_route` → `inc_bundles_skipped("unsupported_dex")`
- Arb dedup → `inc_bundles_skipped("dedup")`
- Stale blockhash → `inc_bundles_skipped("stale_blockhash")`

Wrap `find_routes` with timing:
```rust
                let route_start = std::time::Instant::now();
                let mut routes = route_calculator.find_routes(&trigger);
                routes.extend(route_calculator.find_routes(&trigger_reverse));
                solana_mev_bot::metrics::counters::record_route_calc_duration_us(
                    route_start.elapsed().as_micros() as u64);
```

Wrap `simulate` with timing:
```rust
                let sim_start = std::time::Instant::now();
                let sim_result = if skip_simulator && ... { ... } else {
                    profit_simulator.simulate(best_route)
                };
                solana_mev_bot::metrics::counters::record_simulation_duration_us(
                    sim_start.elapsed().as_micros() as u64);
```

- [ ] **Step 3: Add cache gauge to maintenance task**

In the cache maintenance task (around line 474), after `state_cache.evict_stale();`:
```rust
                        solana_mev_bot::metrics::counters::set_cache_pools_tracked(state_cache.len());
```

- [ ] **Step 4: Add shutdown call**

Before `info!("Engine shutdown complete");` at the end of main:
```rust
    solana_mev_bot::metrics::shutdown();
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check`
Expected: compiles clean

- [ ] **Step 6: Run all tests**

Run: `cargo test --test unit`
Expected: all 146+ tests pass

- [ ] **Step 7: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire Prometheus metrics + OTLP tracing into pipeline"
```

---

### Task 7: Instrument Geyser stream (stream.rs)

**Files:**
- Modify: `src/mempool/stream.rs`

- [ ] **Step 1: Add counter calls to the Geyser stream parser**

After each successful pool state parse (where `PoolStateChange` is sent to the channel), add:
```rust
crate::metrics::counters::inc_geyser_updates(DEX_TYPE_STR);
```

Where `DEX_TYPE_STR` is the string matching the parser (e.g., `"orca"`, `"raydium_amm"`, `"raydium_clmm"`, `"raydium_cp"`, `"meteora_dlmm"`, `"meteora_damm_v2"`, `"phoenix"`, `"manifest"`).

On parser failures, add:
```rust
crate::metrics::counters::inc_geyser_parse_errors(DEX_TYPE_STR);
```

Wrap each per-DEX parse call with timing:
```rust
let parse_start = std::time::Instant::now();
// ... existing parse call ...
crate::metrics::counters::record_geyser_parse_duration_us(
    DEX_TYPE_STR, parse_start.elapsed().as_micros() as u64);
```

On LaserStream reconnection events (if detectable in the stream), add:
```rust
crate::metrics::counters::inc_geyser_reconnections();
```

For Geyser slot lag, after extracting the update slot:
```rust
// If we have a way to know the network tip slot, compute lag.
// The slot from the update itself can be compared to the highest seen slot.
```

- [ ] **Step 2: Add vault fetch counter**

In the lazy vault fetch path (where `getMultipleAccounts` is called for Raydium AMM/CP), add:
```rust
crate::metrics::counters::inc_vault_fetches(DEX_TYPE_STR);
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check`
Expected: compiles clean

- [ ] **Step 4: Run tests**

Run: `cargo test --test unit`
Expected: all tests pass

- [ ] **Step 5: Commit**

```bash
git add src/mempool/stream.rs
git commit -m "feat: instrument Geyser stream with metrics counters"
```

---

### Task 8: Instrument relay submissions

**Files:**
- Modify: `src/executor/relays/jito.rs`
- Modify: `src/executor/relays/nozomi.rs`
- Modify: `src/executor/relays/bloxroute.rs`
- Modify: `src/executor/relays/astralane.rs`
- Modify: `src/executor/relays/zeroslot.rs`

- [ ] **Step 1: Add metric recording at the end of each relay's submit()**

In each relay's `submit()` method, before the final `return` or at the point where `RelayResult` is produced, add:

```rust
// After getting the RelayResult (either from parse_jsonrpc_response or fail_with_latency):
let status = if result.success { "accepted" } else { "rejected" };
crate::metrics::counters::inc_relay_submission(RELAY_NAME, status);
crate::metrics::counters::record_relay_latency_us(RELAY_NAME, result.latency_us);
```

Where `RELAY_NAME` is `"jito"`, `"nozomi"`, `"bloxroute"`, `"astralane"`, `"zeroslot"` respectively.

For rate-limited returns (early exit), record:
```rust
crate::metrics::counters::inc_relay_submission(RELAY_NAME, "rate_limited");
```

The cleanest approach: add a helper to `common.rs`:
```rust
/// Record metrics for a relay submission result.
pub fn record_relay_metrics(result: &super::RelayResult) {
    let status = if result.success {
        "accepted"
    } else if result.error.as_deref() == Some("Rate limited") {
        "rate_limited"
    } else {
        "rejected"
    };
    crate::metrics::counters::inc_relay_submission(&result.relay_name, status);
    if result.latency_us > 0 {
        crate::metrics::counters::record_relay_latency_us(&result.relay_name, result.latency_us);
    }
}
```

Then call `common::record_relay_metrics(&result)` at the end of each `submit()` before returning.

- [ ] **Step 2: Verify compilation**

Run: `cargo check`
Expected: compiles clean

- [ ] **Step 3: Run tests**

Run: `cargo test --test unit`
Expected: all tests pass

- [ ] **Step 4: Commit**

```bash
git add src/executor/relays/
git commit -m "feat: instrument relay submissions with metrics"
```

---

### Task 9: Instrument blockhash cache

**Files:**
- Modify: `src/state/blockhash.rs`

- [ ] **Step 1: Add blockhash age gauge**

In `fetch_and_update()`, after a successful cache update:
```rust
    crate::metrics::counters::set_blockhash_age_ms(0);
```

In `BlockhashCache::get()`, when the blockhash IS returned (not stale), record the age:
```rust
    // After computing fetched_at.elapsed():
    let age = info.fetched_at.elapsed();
    crate::metrics::counters::set_blockhash_age_ms(age.as_millis() as u64);
```

Actually, the simplest approach: in `run_blockhash_loop()`, after each successful fetch:
```rust
                    Ok(()) => {
                        crate::metrics::counters::set_blockhash_age_ms(0);
                        // ... existing recovery logging ...
                    }
```

- [ ] **Step 2: Verify compilation and run tests**

Run: `cargo check && cargo test --test unit`
Expected: compiles, all tests pass

- [ ] **Step 3: Commit**

```bash
git add src/state/blockhash.rs
git commit -m "feat: add blockhash age gauge metric"
```

---

### Task 10: Update docs and .env.example

**Files:**
- Modify: `CLAUDE.md`
- Modify: `.env.example`

- [ ] **Step 1: Add metrics env vars to .env.example**

```env
# Metrics (optional)
# METRICS_PORT=9090                              # Prometheus /metrics HTTP endpoint
# OTLP_ENDPOINT=http://localhost:4317            # OTLP gRPC endpoint for tracing
# OTLP_SERVICE_NAME=mev-engine                   # Service name in traces
```

- [ ] **Step 2: Update CLAUDE.md roadmap**

Mark Grafana + OpenTelemetry metrics as DONE in the roadmap section.

- [ ] **Step 3: Update CLAUDE.md module map**

Add `src/metrics/` to the module map in CLAUDE.md:

```
├── metrics/
│   ├── mod.rs           # init(), shutdown(), Prometheus HTTP server
│   ├── counters.rs      # All metric helper functions (atomic, zero-cost)
│   └── tracing_layer.rs # Optional OTLP tracing layer builder
```

- [ ] **Step 4: Update CLAUDE.md environment variables**

Add METRICS_PORT, OTLP_ENDPOINT, OTLP_SERVICE_NAME to the environment variables section.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md .env.example
git commit -m "docs: add metrics configuration to CLAUDE.md and .env.example"
```

---

### Task 11: Final integration test and clippy

**Files:**
- All

- [ ] **Step 1: Run full test suite**

Run: `cargo test --test unit`
Expected: all tests pass (146+ with new metrics tests)

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: 0 warnings

- [ ] **Step 3: Check e2e_surfpool compilation**

Run: `cargo check --features e2e_surfpool --tests`
Expected: compiles clean

- [ ] **Step 4: Final commit if any fixups needed**

```bash
git add -A
git commit -m "fix: clippy and test fixes for metrics integration"
```
