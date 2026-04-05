# Prometheus + OTLP Metrics Design

**Date:** 2026-04-05
**Status:** Approved
**Approach:** B — Prometheus scrape for metrics + OTLP push for tracing spans

## Goal

Production observability for the MEV engine without impacting hot path latency. Two telemetry pipelines:

1. **Prometheus metrics** — counters, gauges, histograms exposed via HTTP `/metrics` endpoint for Grafana dashboards
2. **OTLP tracing spans** — exported to Grafana Tempo / Jaeger for latency profiling and flame graphs

Both are optional: disabled when env vars are not set, zero overhead in dev.

## Non-Goals

- OTel log export (JSON logs already work fine)
- Distributed tracing correlation (single binary, no service mesh)
- Per-pool-address metrics (high cardinality, stays in logs)

## Architecture

### Module Layout

```
src/metrics/
  mod.rs          — init(), shutdown(), re-exports
  counters.rs     — All metric definitions (free functions wrapping metrics macros)
  tracing.rs      — OTel tracing layer setup (OTLP span export)
```

### Initialization (main.rs)

```
1. metrics::init(&config)          — register Prometheus recorder + OTel tracing layer
2. tokio::spawn(metrics HTTP)      — serve /metrics on METRICS_PORT
3. ... existing pipeline ...
4. metrics::shutdown()             — flush pending spans on graceful shutdown
```

### Hot Path Cost

The router thread (sync, CPU-bound) is the hot path:

```
recv → dedup → cache lookup → find_routes → simulate → build_ixs → relay dispatch
```

Instrumentation cost per iteration:
- **Counters/histograms:** atomic increment via `metrics` crate global recorder (~1-2ns each)
- **Tracing spans:** thread-local span creation via `tracing` crate (~100-200ns each)
- **Export:** background threads only. Prometheus: scraped by Grafana (no hot path involvement). OTLP: batched span export on async thread.

Total overhead: ~500ns per hot path iteration. At 1000 iterations/sec that's 0.5ms/sec — negligible.

### Design Rules

1. No `Arc<Metrics>` passed around. The `metrics` crate uses a global recorder set once at init. Call `metrics::counter!()` anywhere.
2. `counters.rs` exposes typed free functions (e.g., `record_opportunity(profit, hops)`). Hot path calls these.
3. Tracing spans use `tracing::info_span!()` — same crate already in deps. OTel layer added as second subscriber layer alongside JSON logger.
4. Low-cardinality attributes only on spans: `dex_type`, `relay`, `hops`, `profitable`. Pool addresses stay in log events.
5. Histogram buckets tuned for microseconds: `[10, 50, 100, 250, 500, 1000, 2500, 5000, 10000, 50000]`.

## Dependencies

```toml
# Prometheus metrics
metrics = "0.24"
metrics-exporter-prometheus = "0.16"

# OpenTelemetry tracing (optional, enabled by OTLP_ENDPOINT env var)
opentelemetry = "0.28"
opentelemetry_sdk = { version = "0.28", features = ["rt-tokio"] }
opentelemetry-otlp = "0.28"
tracing-opentelemetry = "0.29"
```

## Configuration (Environment Variables)

| Variable | Default | Description |
|----------|---------|-------------|
| `METRICS_PORT` | (disabled) | Port for Prometheus `/metrics` HTTP endpoint |
| `OTLP_ENDPOINT` | (disabled) | OTLP gRPC endpoint (e.g., `http://localhost:4317`) |
| `OTLP_SERVICE_NAME` | `mev-engine` | Service name in traces |

When neither is set, `metrics::init()` is a no-op and no background threads are spawned.

## Metrics Definitions

### Counters

| Metric | Labels | Location |
|--------|--------|----------|
| `geyser_updates_total` | `dex_type` | `stream.rs` — each parsed pool state change |
| `geyser_parse_errors_total` | `dex_type` | `stream.rs` — parser failures |
| `geyser_reconnections_total` | — | `stream.rs` — LaserStream reconnect events |
| `routes_found_total` | `hops` | `main.rs` — after `find_routes()` returns non-empty |
| `opportunities_total` | `dex_type` | `main.rs` — simulator confirms profitable |
| `bundles_submitted_total` | — | `main.rs` — after relay dispatch |
| `bundles_skipped_total` | `reason` | `main.rs` — dry_run, dedup, unsupported_dex, stale_blockhash |
| `relay_submissions_total` | `relay`, `status` | Each relay's `submit()` — accepted/rejected/error |
| `bundle_build_errors_total` | — | `main.rs` — `build_arb_instructions` failure |
| `profit_lamports_total` | — | `main.rs` — sum of final_profit_lamports |
| `tips_paid_lamports_total` | — | `main.rs` — sum of tip_lamports |
| `vault_fetches_total` | `dex_type` | `stream.rs` — lazy vault/tick/bin RPC calls |

### Gauges

| Metric | Location |
|--------|----------|
| `cache_pools_tracked` | `main.rs` cache maintenance task (30s interval) |
| `geyser_lag_slots` | `stream.rs` — `network_slot - update.slot` |
| `channel_backpressure` | `main.rs` — `channel.len()` sampled periodically |
| `blockhash_age_ms` | `blockhash.rs` — ms since last successful fetch |

### Histograms

| Metric | Labels | Location |
|--------|--------|----------|
| `route_calc_duration_us` | — | `main.rs` — wraps `find_routes()` both directions |
| `simulation_duration_us` | — | `main.rs` — wraps `simulate()` |
| `relay_latency_us` | `relay` | Each relay's `submit()` (already tracked internally) |
| `geyser_parse_duration_us` | `dex_type` | `stream.rs` — per-parser timing |

## Tracing Spans (OTLP)

Created only when `OTLP_ENDPOINT` is set.

| Span | Attributes | Location |
|------|-----------|----------|
| `process_state_change` | `pool` (short), `slot`, `dex_type` | `main.rs` — full iteration |
| `find_routes` | `trigger_pool` (short), `routes_found` | `main.rs` — both direction calls |
| `simulate` | `hops`, `profitable` | `main.rs` — simulator call |
| `build_bundle` | `hops`, `dex_types` | `main.rs` — IX builder |
| `relay_submit` | `relay`, `success` | Each relay `submit()` |

## Subscriber Stack (main.rs tracing init)

Current: single JSON fmt layer.

New: layered subscriber with optional OTel:

```
tracing_subscriber::registry()
    .with(json_fmt_layer)            // existing — logs to stdout
    .with(otel_layer_if_configured)  // new — span export to OTLP
    .init()
```

The `metrics` crate is independent of `tracing` — it has its own global recorder.

## Files Modified

| File | Change |
|------|--------|
| `Cargo.toml` | Add metrics + opentelemetry deps |
| `src/lib.rs` | Add `pub mod metrics;` |
| `src/metrics/mod.rs` | NEW — init/shutdown, HTTP server |
| `src/metrics/counters.rs` | NEW — metric definitions + helper functions |
| `src/metrics/tracing.rs` | NEW — OTel tracing layer builder |
| `src/main.rs` | Call `metrics::init()`, add counter/histogram calls on hot path, update tracing subscriber |
| `src/mempool/stream.rs` | Add geyser_updates_total, parse_errors, parse_duration counters |
| `src/executor/relays/*.rs` | Add relay_submissions_total, relay_latency_us recording |
| `src/state/blockhash.rs` | Add blockhash_age_ms gauge |
| `src/config.rs` | Add METRICS_PORT, OTLP_ENDPOINT, OTLP_SERVICE_NAME to BotConfig |

## Testing

- Unit test: `metrics::init()` with no env vars → no-op, no panic
- Unit test: counter/histogram helper functions callable without init (metrics crate handles gracefully)
- Integration test: start metrics server, scrape `/metrics`, verify expected metric names present
- Manual: run with `METRICS_PORT=9090`, curl `localhost:9090/metrics`, verify Prometheus format output
