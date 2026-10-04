//! Telemetry bootstrap: structured logs + spans via `tracing`, distributed traces via
//! OpenTelemetry (W3C Trace Context propagation, optional OTLP export), metrics via the
//! `metrics` facade exported in Prometheus text format.
//!
//! Trace context flows request → job → worker → provider: the HTTP span adopts an inbound
//! `traceparent`, job rows carry the context ([`propagation::current_context_map`]), workers
//! resume it, and the outbound engine injects `traceparent` into provider calls.
//!
//! Conventions (see docs/operations/observability.md):
//! - Metric names are `app_<subsystem>_<thing>_<unit>`; labels are low-cardinality
//!   (route templates, provider names, status classes; never ids or raw paths).
//! - Never log request/response bodies, tokens, cookies or provider payloads.

pub mod propagation;

use app_config::{LogFormat, TelemetryConfig};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::{
    Resource,
    propagation::TraceContextPropagator,
    trace::{Sampler, SdkTracerProvider},
};
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

pub use metrics_exporter_prometheus::PrometheusHandle as MetricsHandle;

#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    #[error("invalid log filter: {0}")]
    Filter(String),
    #[error("metrics recorder: {0}")]
    Metrics(String),
    #[error("tracing subscriber already installed")]
    AlreadyInstalled,
    #[error("OTLP exporter: {0}")]
    Exporter(String),
}

/// Keeps the trace pipeline alive; dropping it flushes and shuts down the exporter.
#[must_use = "dropping the guard shuts down trace export"]
pub struct TelemetryGuard {
    provider: Option<SdkTracerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(p) = self.provider.take()
            && let Err(e) = p.shutdown()
        {
            eprintln!("trace exporter shutdown: {e}");
        }
    }
}

/// Build the OpenTelemetry tracer provider when tracing is wanted: an OTLP endpoint is set
/// (export), or `trace_propagation` is on (trace ids are created and propagated, nothing is
/// exported). Installs the W3C Trace Context propagator globally.
fn otel_provider(cfg: &TelemetryConfig) -> Result<Option<SdkTracerProvider>, TelemetryError> {
    if cfg.otlp_endpoint.is_empty() && !cfg.trace_propagation {
        return Ok(None);
    }
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
    let ratio = cfg.trace_sample_ratio.clamp(0.0, 1.0);
    let mut b = SdkTracerProvider::builder()
        // Respect the caller's sampling decision; sample new root traces at `ratio`.
        .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(ratio))))
        .with_resource(Resource::builder().with_service_name(cfg.service_name.clone()).build());
    if !cfg.otlp_endpoint.is_empty() {
        use opentelemetry_otlp::WithExportConfig as _;
        let base = cfg.otlp_endpoint.trim_end_matches('/');
        let url = if base.ends_with("/v1/traces") { base.to_string() } else { format!("{base}/v1/traces") };
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(url)
            .build()
            .map_err(|e| TelemetryError::Exporter(e.to_string()))?;
        b = b.with_batch_exporter(exporter);
    }
    let provider = b.build();
    opentelemetry::global::set_tracer_provider(provider.clone());
    Ok(Some(provider))
}

/// Latency buckets (seconds) spanning sub-millisecond handlers to slow upstream calls.
pub const LATENCY_BUCKETS: &[f64] =
    &[0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0];

/// Install the global tracing subscriber (logs + OpenTelemetry). `RUST_LOG`, when set,
/// overrides `filter`. Keep the returned guard alive for the life of the process.
pub fn init_tracing(filter: &str, format: LogFormat, cfg: &TelemetryConfig) -> Result<TelemetryGuard, TelemetryError> {
    let env_filter = match std::env::var("RUST_LOG") {
        Ok(v) if !v.is_empty() => EnvFilter::try_new(v),
        _ => EnvFilter::try_new(filter),
    }
    .map_err(|e| TelemetryError::Filter(e.to_string()))?;
    let provider = otel_provider(cfg)?;
    let otel = provider.as_ref().map(|p| tracing_opentelemetry::layer().with_tracer(p.tracer("app")));
    let fmt = match format {
        LogFormat::Json => tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .boxed(),
        LogFormat::Pretty => tracing_subscriber::fmt::layer().compact().boxed(),
    };
    tracing_subscriber::registry()
        .with(env_filter)
        .with(otel)
        .with(fmt)
        .try_init()
        .map_err(|_| TelemetryError::AlreadyInstalled)?;
    Ok(TelemetryGuard { provider })
}

/// Install the global Prometheus recorder and return the handle used to render `/metrics`.
pub fn init_metrics(cfg: &TelemetryConfig) -> Result<PrometheusHandle, TelemetryError> {
    let handle = PrometheusBuilder::new()
        .set_buckets_for_metric(Matcher::Suffix("_seconds".into()), LATENCY_BUCKETS)
        .map_err(|e| TelemetryError::Metrics(e.to_string()))?
        .add_global_label("service", cfg.service_name.clone())
        .install_recorder()
        .map_err(|e| TelemetryError::Metrics(e.to_string()))?;
    describe_metrics();
    Ok(handle)
}

/// A recorder for tests or embedded use that is not installed globally.
pub fn local_metrics_handle() -> PrometheusHandle {
    PrometheusBuilder::new().build_recorder().handle()
}

fn describe_metrics() {
    use metrics::{describe_counter, describe_gauge, describe_histogram};
    describe_counter!("app_http_requests_total", "HTTP requests by method, route and status");
    describe_histogram!("app_http_request_duration_seconds", "HTTP request latency by method and route");
    describe_gauge!("app_http_inflight_requests", "HTTP requests currently being served");
    describe_counter!("app_http_shed_total", "Requests rejected by load shedding");
    describe_counter!("app_rate_limited_total", "Requests rejected by the per-client rate limiter");
}
