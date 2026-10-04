//! Telemetry bootstrap: structured logs + spans via `tracing`, metrics via the `metrics`
//! facade exported in Prometheus text format.
//!
//! Conventions (see docs/operations/observability.md):
//! - Metric names are `app_<subsystem>_<thing>_<unit>`; labels are low-cardinality
//!   (route templates, provider names, status classes; never ids or raw paths).
//! - Never log request/response bodies, tokens, cookies or provider payloads.

use app_config::{LogFormat, TelemetryConfig};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

pub use metrics_exporter_prometheus::PrometheusHandle as MetricsHandle;

#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    #[error("invalid log filter: {0}")]
    Filter(String),
    #[error("metrics recorder: {0}")]
    Metrics(String),
    #[error("tracing subscriber already installed")]
    AlreadyInstalled,
}

/// Latency buckets (seconds) spanning sub-millisecond handlers to slow upstream calls.
pub const LATENCY_BUCKETS: &[f64] =
    &[0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0];

/// Install the global tracing subscriber. `RUST_LOG`, when set, overrides `filter`.
pub fn init_tracing(filter: &str, format: LogFormat) -> Result<(), TelemetryError> {
    let env_filter = match std::env::var("RUST_LOG") {
        Ok(v) if !v.is_empty() => EnvFilter::try_new(v),
        _ => EnvFilter::try_new(filter),
    }
    .map_err(|e| TelemetryError::Filter(e.to_string()))?;
    let registry = tracing_subscriber::registry().with(env_filter);
    let res = match format {
        LogFormat::Json => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .flatten_event(true)
                    .with_current_span(true)
                    .with_span_list(false),
            )
            .try_init(),
        LogFormat::Pretty => registry.with(tracing_subscriber::fmt::layer().compact()).try_init(),
    };
    res.map_err(|_| TelemetryError::AlreadyInstalled)
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
