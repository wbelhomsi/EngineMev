//! Optional OpenTelemetry tracing layer for OTLP span export.
//!
//! When OTLP_ENDPOINT is set, builds a tracing layer that exports
//! spans to an OTLP-compatible collector (Grafana Tempo, Jaeger, etc.).
//! Spans are batched and exported on a background thread by the SDK.
//!
//! Uses HTTP/protobuf transport (default feature of opentelemetry-otlp).
//! The endpoint should be the OTLP HTTP receiver, typically port 4318.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::registry::LookupSpan;

/// Build an OpenTelemetry tracing layer that exports spans via OTLP HTTP/protobuf.
///
/// Returns `Some((layer, provider))` on success, or `None` if building fails.
/// The caller must keep `provider` alive for the duration of the application.
/// Dropping it triggers flush + shutdown of the span pipeline.
///
/// # Arguments
/// * `endpoint` - OTLP HTTP endpoint (e.g. `http://localhost:4318`)
/// * `service_name` - Value for the `service.name` resource attribute
pub fn build_layer<S>(
    endpoint: &str,
    service_name: &str,
) -> Option<(OpenTelemetryLayer<S, opentelemetry_sdk::trace::SdkTracer>, SdkTracerProvider)>
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span>,
{
    // 1. Build the OTLP span exporter using HTTP/protobuf transport.
    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()
    {
        Ok(exp) => exp,
        Err(e) => {
            tracing::error!("Failed to build OTLP span exporter: {}", e);
            return None;
        }
    };

    // 2. Build a Resource with the service name.
    let resource = Resource::builder_empty()
        .with_service_name(service_name.to_string())
        .build();

    // 3. Create a TracerProvider with batch processing and the resource.
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    // 4. Create a tracer from the provider and build the tracing layer.
    let tracer = provider.tracer("engine-mev");
    let layer = tracing_opentelemetry::layer().with_tracer(tracer);

    Some((layer, provider))
}
