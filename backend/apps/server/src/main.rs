//! `app-server`: the API process. Everything optional is decided by configuration
//! (and, for heavy adapters, cargo features) so the same binary serves every profile.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Context;
use app_api::{AppState, BuildInfo};
use app_config::{AppConfig, CacheBackend};
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
    // Kept until main returns: dropping it flushes pending trace exports.
    let _telemetry = app_telemetry::init_tracing(&config.log.filter, config.log.format, &config.telemetry)?;
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
    // Cache: in-memory (core) or Redis/Dragonfly (performance profile). Redis is optional
    // infrastructure: if it is down at startup we log and continue with the in-memory cache.
    let redis = if config.cache.backend == CacheBackend::Redis || config.rate_limit.backend == CacheBackend::Redis {
        match app_cache::redis::RedisCache::connect(config.cache.redis_url.expose()).await {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::error!(error = %e, "Redis unavailable at startup; using in-memory cache/limits");
                None
            }
        }
    } else {
        None
    };
    let cache_backend: Arc<dyn app_cache::Cache> = match (&redis, config.cache.backend) {
        (Some(r), CacheBackend::Redis) => Arc::new(r.clone()),
        _ => Arc::new(app_cache::memory::MemoryCache::new(config.cache.memory_max_entries)),
    };
    if let Some(r) = &redis {
        builder = builder.health_check(Arc::new(RedisCheck(r.clone())));
    }
    builder = builder.cache(app_cache::CacheLayer::new(cache_backend, &config.cache.namespace));
    if config.rate_limit.enabled {
        match (&redis, config.rate_limit.backend) {
            (Some(r), CacheBackend::Redis) => {
                builder = builder.rate_limiter(Arc::new(app_cache::redis::RedisRateLimiter::new(
                    r,
                    &config.cache.namespace,
                    config.rate_limit.per_client_rps,
                    config.rate_limit.burst,
                )));
            }
            _ => {
                let limiter = Arc::new(MemoryRateLimiter::new(Quota {
                    per_second: config.rate_limit.per_client_rps,
                    burst: config.rate_limit.burst,
                }));
                limiter.spawn_janitor(Duration::from_secs(60));
                builder = builder.rate_limiter(limiter);
            }
        }
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

    // Outbound providers and realtime fan-out (PostgreSQL NOTIFY works across processes).
    let providers = app_networking::ProviderRegistry::from_config(&config.providers)?;
    let events = app_workers::events::start_event_bus(&config.messaging, pool.clone(), "app-server")
        .await
        .map_err(anyhow::Error::msg)
        .context("starting event bus")?;
    let analytics = app_analytics::start(&config.analytics).await;
    if let Some(q) = &analytics.query {
        builder = builder.health_check(Arc::new(AnalyticsCheck(q.clone()))).analytics_query(q.clone());
    }
    builder = builder.analytics(analytics.sink.clone());
    let health_registry = providers.clone();
    builder = builder
        .health_check(Arc::new(app_api::services::DbCheck(pool.clone())))
        .health_check(Arc::new(app_api::services::IdpCheck(services.oidc.clone())))
        .events(events.clone())
        .provider_health(Arc::new(move |name: &str| {
            health_registry.health(name).map(|h| app_api::dto::ProviderHealth {
                concurrency_limit: h.concurrency_limit,
                inflight: h.inflight,
                queued: h.queued,
                circuit: h.circuit.to_string(),
                success_rate: h.success_rate,
                p50_ms: h.p50_ms,
                p95_ms: h.p95_ms,
                p99_ms: h.p99_ms,
                rate_429: h.rate_429,
            })
        }))
        .services(services);
    let state = builder.build();
    let router = app_api::build_router(state.clone());

    // Simple deployments run the job worker in this process; larger ones run `app-worker`.
    let worker = config.jobs.run_in_process.then(|| {
        let mut wc = app_workers::WorkerConfig::new("runs", config.jobs.concurrency);
        wc.poll_interval = Duration::from_millis(config.jobs.poll_interval_ms);
        let svc = app_workers::JobServices {
            db: pool.clone(),
            events: events.clone(),
            providers: providers.clone(),
            analytics: analytics.sink.clone(),
        };
        let w =
            app_workers::PgWorker::new(wc, svc, vec![Arc::new(app_workers::handlers::ExecuteRun { parallelism: 16 })]);
        tokio::spawn(w.run(state.lifecycle.shutdown.clone()))
    });

    let addr = SocketAddr::new(config.http.host, config.http.port);
    let listener = tokio::net::TcpListener::bind(addr).await.with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "listening");
    app_api::server::serve(listener, router, state).await?;
    if let Some(w) = worker {
        let _ = w.await; // the worker drains in-flight jobs (bounded) on the same shutdown signal
    }
    analytics.shutdown().await; // flush buffered analytics events
    Ok(())
}

/// ClickHouse analytics is never critical: failures report "degraded".
struct AnalyticsCheck(Arc<app_analytics::AnalyticsQuery>);

#[async_trait::async_trait]
impl app_api::health::HealthCheck for AnalyticsCheck {
    fn name(&self) -> &'static str {
        "analytics"
    }
    fn critical(&self) -> bool {
        false
    }
    async fn check(&self) -> Result<(), String> {
        self.0.ping().await.map_err(|e| e.to_string())
    }
}

/// Redis is a performance optimisation, never critical: failures report "degraded".
struct RedisCheck(app_cache::redis::RedisCache);

#[async_trait::async_trait]
impl app_api::health::HealthCheck for RedisCheck {
    fn name(&self) -> &'static str {
        "redis"
    }
    fn critical(&self) -> bool {
        false
    }
    async fn check(&self) -> Result<(), String> {
        app_cache::Cache::ping(&self.0).await.map_err(|e| e.to_string())
    }
}
