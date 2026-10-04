//! `app-server`: the API process. Everything optional is decided by configuration
//! (and, for heavy adapters, cargo features) so the same binary serves every profile.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Context;
use app_api::{AppState, BuildInfo};
use app_config::AppConfig;
use app_rate_limit::{MemoryRateLimiter, Quota};

/// `app-server check-config [--online]`: validate configuration without starting the server.
/// Offline: every rule in `AppConfig::validate` (redirect URIs, cookie policy, production
/// secrets, ...). Online: also fetch OIDC discovery and verify the issuer and required endpoints.
async fn check_config(online: bool) -> anyhow::Result<()> {
    let config = AppConfig::load().context("configuration invalid")?;
    println!("configuration: OK (environment={}, provider={:?})", config.environment, config.auth.provider);
    if online {
        let url = format!("{}/.well-known/openid-configuration", config.auth.issuer_url.trim_end_matches('/'));
        let body: serde_json::Value = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()?
            .get(&url)
            .send()
            .await
            .with_context(|| format!("OIDC discovery unreachable: {url}"))?
            .error_for_status()?
            .json()
            .await
            .context("OIDC discovery is not JSON")?;
        let issuer = body["issuer"].as_str().unwrap_or_default();
        anyhow::ensure!(
            issuer == config.auth.issuer_url.trim_end_matches('/'),
            "issuer mismatch: discovery says {issuer:?}"
        );
        for key in ["authorization_endpoint", "token_endpoint", "jwks_uri"] {
            anyhow::ensure!(body[key].is_string(), "discovery lacks {key}");
        }
        let pkce = body["code_challenge_methods_supported"].as_array().is_none_or(|m| m.iter().any(|v| v == "S256"));
        anyhow::ensure!(pkce, "provider does not advertise PKCE S256");
        if body["end_session_endpoint"].is_null() {
            println!("warning: provider has no end_session_endpoint; logout ends only the local session");
        }
        println!("identity provider: OK ({issuer})");
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("check-config") {
        return check_config(args.iter().any(|a| a == "--online")).await;
    }
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
    // PostgreSQL is the core of every profile.
    let pool = app_db::connect(&config.database, "app-server").await?;
    if config.database.migrate_on_start {
        app_db::migrate(&pool).await.context("running migrations")?;
        tracing::info!("migrations applied");
    }
    if let Err(e) = app_db::orgs::sync_permissions(&pool).await {
        tracing::warn!(error = %e, "permission vocabulary sync failed (database unavailable?); will be retried on next start");
    }
    let services = Arc::new(app_api::bootstrap::build_services(&config, pool.clone())?);
    builder = builder
        .health_check(Arc::new(app_api::services::DbCheck(pool.clone())))
        .health_check(Arc::new(app_api::services::IdpCheck(services.oidc.clone())))
        .services(services);
    let state = builder.build();
    let router = app_api::build_router(state.clone());

    let addr = SocketAddr::new(config.http.host, config.http.port);
    let listener = tokio::net::TcpListener::bind(addr).await.with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "listening");
    app_api::server::serve(listener, router, state).await?;
    Ok(())
}
