//! `app-server`: the API process. Everything optional is decided by configuration
//! (and, for heavy adapters, cargo features) so the same binary serves every profile.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Context;
use app_api::{AppState, BuildInfo};
use app_config::AppConfig;
use app_rate_limit::{MemoryRateLimiter, Quota};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = AppConfig::load().context("loading configuration")?;
    app_telemetry::init_tracing(&config.log.filter, config.log.format)?;
    let build = BuildInfo {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
        git_sha: env!("APP_GIT_SHA"),
        profile: if cfg!(debug_assertions) { "debug" } else { "release" },
    };
    tracing::info!(version = build.version, git_sha = build.git_sha, env = %config.environment, "starting");

    let mut builder = AppState::builder(config.clone(), build);
    if config.telemetry.metrics {
        builder = builder.metrics(app_telemetry::init_metrics(&config.telemetry)?);
    }
    if config.rate_limit.enabled {
        let limiter = Arc::new(MemoryRateLimiter::new(Quota {
            per_second: config.rate_limit.per_client_rps,
            burst: config.rate_limit.burst,
        }));
        limiter.spawn_janitor(Duration::from_secs(60));
        builder = builder.rate_limiter(limiter);
    }
    let state = builder.build();
    let router = app_api::build_router(state.clone());

    let addr = SocketAddr::new(config.http.host, config.http.port);
    let listener = tokio::net::TcpListener::bind(addr).await.with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "listening");
    app_api::server::serve(listener, router, state).await?;
    Ok(())
}
