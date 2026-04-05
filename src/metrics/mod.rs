//! Production observability: Prometheus metrics + optional OTLP tracing.
//!
//! Both pipelines are optional:
//! - Set METRICS_PORT to enable Prometheus /metrics HTTP endpoint
//! - Set OTLP_ENDPOINT to enable tracing span export
//!
//! When neither is set, init() is a near-no-op.

pub mod counters;
pub mod tracing_layer;

use metrics_exporter_prometheus::PrometheusBuilder;
use tracing::info;

/// Initialize the Prometheus metrics recorder.
/// If `metrics_port` is Some, installs a Prometheus recorder with HTTP listener.
/// If None, no recorder is installed (counter calls become no-ops).
pub fn init(metrics_port: Option<u16>, otlp_endpoint: Option<&str>, service_name: &str) {
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
/// In OTel SDK 0.31+, shutdown is handled by dropping the `SdkTracerProvider`
/// returned from `tracing_layer::build_layer`. The caller should store the provider
/// and call `provider.shutdown()` or let it drop at application exit.
pub fn shutdown() {
    // No global shutdown function in OTel 0.31.
    // SdkTracerProvider::drop triggers flush + shutdown automatically.
}
