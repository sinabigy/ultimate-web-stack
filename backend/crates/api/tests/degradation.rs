#![allow(clippy::unwrap_used)]
//! Failure engineering: the API degrades predictably when PostgreSQL or the IdP is down.

use std::sync::Arc;

use app_api::{AppState, BuildInfo, services::DbCheck};
use app_config::{AppConfig, IdentityProviderKind, Secret};
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn app_with_dead_dependencies() -> (axum::Router, AppState) {
    let mut cfg = AppConfig { environment: "test".into(), ..Default::default() };
    cfg.rate_limit.enabled = false;
    cfg.auth.provider = IdentityProviderKind::Oidc;
    cfg.auth.issuer_url = "http://127.0.0.1:1".into(); // nothing listens on port 1
    // Unreachable database with a short acquire timeout.
    cfg.database.url = Secret::new("postgres://app:x@127.0.0.1:1/app");
    cfg.database.acquire_timeout_ms = 300;
    let pool = app_db::connect(&cfg.database, "test").await.unwrap();
    let services = app_api::bootstrap::build_services(&cfg, pool.clone()).unwrap();
    let services = Arc::new(services);
    let state = AppState::builder(cfg, BuildInfo { name: "t", version: "0", git_sha: "t", profile: "debug" })
        .health_check(Arc::new(DbCheck(pool)))
        .health_check(Arc::new(app_api::services::IdpCheck(services.oidc.clone())))
        .services(services)
        .build();
    (app_api::build_router(state.clone()), state)
}

async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, serde_json::Value, axum::http::HeaderMap) {
    let res = app.clone().oneshot(req).await.unwrap();
    let (p, b) = res.into_parts();
    let bytes = b.collect().await.unwrap().to_bytes();
    (p.status, serde_json::from_slice(&bytes).unwrap_or_default(), p.headers)
}

#[tokio::test]
async fn database_outage_yields_503_not_crash() {
    let (app, _) = app_with_dead_dependencies().await;
    // liveness unaffected
    assert_eq!(send(&app, Request::get("/healthz").body(Body::empty()).unwrap()).await.0, StatusCode::OK);
    // readiness fails on the critical dependency and reports the IdP as degraded
    let (status, body, _) = send(&app, Request::get("/readyz").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let checks = body["checks"].as_array().unwrap();
    assert!(checks.iter().any(|c| c["name"] == "postgres" && c["ok"] == false && c["critical"] == true));
    assert!(checks.iter().any(|c| c["name"] == "identity_provider" && c["ok"] == false && c["critical"] == false));
    // an authenticated request needing the database gets a clean, retryable 503
    let token = "A".repeat(43);
    let (status, body, _) = send(
        &app,
        Request::get("/api/v1/dashboard")
            .header(header::COOKIE, format!("app_session={token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "unavailable");
    // no credentials → still a clean 401 (no database needed to say so)
    assert_eq!(
        send(&app, Request::get("/api/v1/dashboard").body(Body::empty()).unwrap()).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn identity_provider_outage_sends_browser_back_to_login_with_reason() {
    let (app, _) = app_with_dead_dependencies().await;
    let (status, _, headers) = send(&app, Request::get("/auth/login").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/login?error=idp_unavailable");
}
