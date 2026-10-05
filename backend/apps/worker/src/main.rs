//! `app-worker`: dedicated background worker. Same configuration as the API (`APP__*`); set
//! `APP__JOBS__RUN_IN_PROCESS=false` on API instances when running this separately.
//! Health and metrics on `APP__WORKER__PORT` (default 9091).

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use app_config::AppConfig;
use axum::{Router, routing::get};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        let port = std::env::var("APP__WORKER__PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(9091);
        let res = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()?
            .get(format!("http://127.0.0.1:{port}/readyz"))
            .send()
            .await?;
        anyhow::ensure!(res.status().is_success(), "worker not ready: {}", res.status());
        return Ok(());
    }
    let config = AppConfig::load().context("loading configuration")?;
    let mut telemetry_cfg = config.telemetry.clone();
    if telemetry_cfg.service_name == "app" {
        telemetry_cfg.service_name = "app-worker".into();
    }
    let _telemetry = app_telemetry::init_tracing(&config.log.filter, config.log.format, &telemetry_cfg)?;
    let metrics = app_telemetry::init_metrics(&config.telemetry)?;
    let pool = app_db::connect(&config.database, "app-worker").await?;
    let providers = app_networking::ProviderRegistry::from_config(&config.providers)?;
    let events = app_workers::events::start_event_bus(&config.messaging, pool.clone(), "app-worker")
        .await
        .map_err(anyhow::Error::msg)
        .context("event bus")?;
    let shutdown = CancellationToken::new();

    let mut wc = app_workers::WorkerConfig::new("runs", config.jobs.concurrency);
    wc.poll_interval = Duration::from_millis(config.jobs.poll_interval_ms);
    let analytics = app_analytics::start(&config.analytics).await.map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let svc = app_workers::JobServices { db: pool.clone(), events, providers, analytics: analytics.sink.clone() };
    let worker =
        app_workers::PgWorker::new(wc, svc, vec![Arc::new(app_workers::handlers::ExecuteRun { parallelism: 16 })]);
    tracing::info!(id = worker.id(), "app-worker starting");
    let runner = tokio::spawn(worker.run(shutdown.clone()));

    let port: u16 = std::env::var("APP__WORKER__PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(9091);
    let probe_pool = pool.clone();
    let ops = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/readyz",
            get(move || {
                let p = probe_pool.clone();
                async move {
                    if app_db::ping(&p).await.is_ok() {
                        (axum::http::StatusCode::OK, "ready")
                    } else {
                        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "db unavailable")
                    }
                }
            }),
        )
        .route(
            "/metrics",
            get(move || {
                let m = metrics.clone();
                async move { m.render() }
            }),
        );
    let listener = tokio::net::TcpListener::bind((config.http.host, port)).await?;
    let stop = shutdown.clone();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, ops).with_graceful_shutdown(async move { stop.cancelled().await }).await;
    });

    app_api::server::shutdown_signal().await;
    tracing::info!("shutdown requested: draining jobs");
    shutdown.cancel();
    let _ = runner.await;
    analytics.shutdown().await; // flush buffered analytics events
    let _ = server.await;
    Ok(())
}
