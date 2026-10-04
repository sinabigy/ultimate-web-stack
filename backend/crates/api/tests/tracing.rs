#![allow(clippy::unwrap_used)]
//! Distributed tracing: one W3C trace from the inbound request, through the job queue and the
//! worker, to the external provider. The tracing subscriber is global, so this file is its own
//! test binary.

mod support;

use std::{
    net::SocketAddr,
    sync::{Arc, OnceLock},
    time::Duration,
};

use app_config::{LogFormat, ProviderDefinition, TelemetryConfig};
use app_networking::ProviderRegistry;
use app_workers::{JobServices, PgWorker, WorkerConfig, events::PgEventBus, handlers::ExecuteRun};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use fake_upstream::{Behaviour, Running};
use serde_json::{Value, json};
use sqlx::PgPool;
use support::*;
use tokio_util::sync::CancellationToken;

static METRICS: OnceLock<app_telemetry::MetricsHandle> = OnceLock::new();

/// Global tracing + Prometheus recorder, as the server installs them.
fn init_tracing() -> &'static app_telemetry::MetricsHandle {
    METRICS.get_or_init(|| {
        // Propagation on, nothing exported (the production default without an OTLP endpoint).
        let cfg = TelemetryConfig { trace_propagation: true, ..Default::default() };
        std::mem::forget(
            app_telemetry::init_tracing(
                "warn,app_api=info,app_workers=info,app_networking=info",
                LogFormat::Pretty,
                &cfg,
            )
            .unwrap(),
        );
        app_telemetry::init_metrics(&cfg).unwrap()
    })
}

async fn app_with_provider(pool: PgPool) -> (TestApp, Running) {
    let up = fake_upstream::start(SocketAddr::from(([127, 0, 0, 1], 0)), Behaviour::default()).await.unwrap();
    let url = up.url();
    let app = TestApp::with_config(pool, |c| {
        c.providers.definitions.insert("simulated".into(), ProviderDefinition { base_url: url, ..Default::default() });
    })
    .await;
    (app, up)
}

async fn create_run(app: &TestApp, b: &Browser, slug: &str, traceparent: Option<&str>) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/orgs/{slug}/runs"))
        .header(header::COOKIE, format!("app_session={}", b.session))
        .header("x-csrf-token", &b.csrf)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(tp) = traceparent {
        req = req.header("traceparent", tp);
    }
    let body = json!({"label": "traced", "provider": "simulated", "requested": 3}).to_string();
    let r = app.raw(req.body(Body::from(body)).unwrap()).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
}

/// Runs the queued job with a real worker until the provider has seen `calls` requests.
async fn run_worker_until(app: &TestApp, up: &Running, calls: u64) {
    let providers = ProviderRegistry::from_config(&app.state.config.providers).unwrap();
    let svc =
        JobServices { db: app.pool.clone(), events: PgEventBus::start(app.pool.clone()).await.unwrap(), providers };
    let mut cfg = WorkerConfig::new("runs", 2);
    cfg.poll_interval = Duration::from_millis(50);
    let stop = CancellationToken::new();
    let w = tokio::spawn(PgWorker::new(cfg, svc, vec![Arc::new(ExecuteRun { parallelism: 2 })]).run(stop.clone()));
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while up.upstream.stats().ok < calls && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    stop.cancel();
    w.await.unwrap();
    assert!(up.upstream.stats().ok >= calls, "provider saw {} calls", up.upstream.stats().ok);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn one_trace_from_request_through_queue_and_worker_to_provider(pool: PgPool) {
    init_tracing();
    let (app, up) = app_with_provider(pool).await;
    let owner = app.login("trace@example.com").await;
    let slug = app.personal_slug(&owner).await;

    let trace_id = "4bf92f3577b34da6a3ce929d0e0e4736";
    create_run(&app, &owner, &slug, Some(&format!("00-{trace_id}-00f067aa0ba902b7-01"))).await;

    // queue: the job row carries the caller's trace and the request id
    let tc: Value = sqlx::query_scalar("SELECT trace_context FROM jobs").fetch_one(&app.pool).await.unwrap();
    let tp = tc["traceparent"].as_str().unwrap();
    assert_eq!(tp.split('-').nth(1), Some(trace_id), "{tc}");
    assert_ne!(tp.split('-').nth(2), Some("00f067aa0ba902b7"), "job is a child span, not the caller's span");
    assert!(tc["request_id"].as_str().is_some_and(|r| !r.is_empty()));

    // worker → provider: every provider call continues the same trace
    run_worker_until(&app, &up, 3).await;
    assert_eq!(up.upstream.stats().trace_ids, vec![trace_id.to_string()]);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn requests_without_trace_context_start_a_new_trace_that_still_propagates(pool: PgPool) {
    init_tracing();
    let (app, up) = app_with_provider(pool).await;
    let owner = app.login("root-trace@example.com").await;
    let slug = app.personal_slug(&owner).await;
    create_run(&app, &owner, &slug, None).await;
    let tc: Value = sqlx::query_scalar("SELECT trace_context FROM jobs").fetch_one(&app.pool).await.unwrap();
    let trace_id = tc["traceparent"].as_str().unwrap().split('-').nth(1).unwrap().to_string();
    assert_eq!(trace_id.len(), 32);
    run_worker_until(&app, &up, 3).await;
    assert_eq!(up.upstream.stats().trace_ids, vec![trace_id]);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn metrics_expose_request_job_and_provider_series(pool: PgPool) {
    let metrics = init_tracing();
    let (app, up) = app_with_provider(pool).await;
    let owner = app.login("metrics@example.com").await;
    let slug = app.personal_slug(&owner).await;
    create_run(&app, &owner, &slug, None).await;
    run_worker_until(&app, &up, 3).await;
    // Same exposition the /metrics endpoint serves (route templates, never raw paths or ids).
    let text = metrics.render();
    for needle in [
        r#"app_http_requests_total{service="app",method="POST",route="/api/v1/orgs/{slug}/runs",status="201"}"#,
        r#"app_provider_requests_total{service="app",provider="simulated",outcome="ok"}"#,
        "app_provider_latency_seconds_bucket",
        r#"app_jobs_processed_total{service="app",queue="runs",kind="execute_run",result="succeeded"}"#,
    ] {
        assert!(text.contains(needle), "missing {needle}");
    }
    assert!(!text.contains(&slug), "no tenant identifiers in metric labels");
}
