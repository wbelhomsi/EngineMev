//! Optional OpenTelemetry tracing layer for OTLP span export.
//!
//! When OTLP_ENDPOINT is set, builds a tracing layer that exports
//! spans to an OTLP-compatible collector (Grafana Tempo, Jaeger, etc.).
//! Spans are batched and exported on a background tokio task.
