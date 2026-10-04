#![allow(clippy::unwrap_used)]
//! T-0002 acceptance: ops endpoints, request ids, structured errors, limits, headers,
//! CSRF, rate limiting, load shedding, graceful drain.

mod common;

use std::{sync::Arc, time::Duration};

use app_rate_limit::{MemoryRateLimiter, Quota};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    routing::{get, post},
};
use common::*;

#[tokio::test]
async fn healthz_and_version() {
    let app = app_api::build_router(state_with(test_config(), vec![]));
    let res = send(&app, get_req("/healthz")).await;
    assert_eq!(res.status(), StatusCode::OK);
    let res = send(&app, get_req("/version")).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(res).await["version"], "0.0.0-test");
}

#[tokio::test]
async fn readyz_reflects_critical_and_noncritical_checks() {
    let ok = state_with(test_config(), vec![Arc::new(StaticCheck("db", true, Ok(())))]);
    assert_eq!(send(&app_api::build_router(ok), get_req("/readyz")).await.status(), StatusCode::OK);

    let degraded = state_with(test_config(), vec![Arc::new(StaticCheck("cache", false, Err("down".into())))]);
    let res = send(&app_api::build_router(degraded), get_req("/readyz")).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(res).await["status"], "degraded");

    let down = state_with(test_config(), vec![Arc::new(StaticCheck("db", true, Err("refused".into())))]);
    let res = send(&app_api::build_router(down), get_req("/readyz")).await;
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body_json(res).await["checks"][0]["error"], "refused");
}

#[tokio::test]
async fn readyz_fails_while_draining() {
    let state = state_with(test_config(), vec![]);
    let app = app_api::build_router(state.clone());
    state.lifecycle.start_draining();
    let res = send(&app, get_req("/readyz")).await;
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(send(&app, get_req("/healthz")).await.status(), StatusCode::OK, "liveness unaffected");
}

#[tokio::test]
async fn request_id_generated_and_propagated() {
    let app = app_api::build_router(state_with(test_config(), vec![]));
    let res = send(&app, get_req("/healthz")).await;
    let rid = res.headers().get("x-request-id").expect("generated").to_str().unwrap().to_string();
    assert_eq!(rid.len(), 36);
    let req = Request::builder().uri("/healthz").header("x-request-id", "abc-123").body(Body::empty()).unwrap();
    assert_eq!(send(&app, req).await.headers()["x-request-id"], "abc-123");
}

#[tokio::test]
async fn unknown_api_route_is_problem_json() {
    let app = app_api::build_router(state_with(test_config(), vec![]));
    let res = send(&app, get_req("/api/v1/nope")).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "application/problem+json");
    assert_eq!(body_json(res).await["code"], "not_found");
}

#[tokio::test]
async fn security_headers_present() {
    let app = app_api::build_router(state_with(test_config(), vec![]));
    let res = send(&app, get_req("/healthz")).await;
    let h = res.headers();
    assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(h[header::X_FRAME_OPTIONS], "DENY");
    assert!(h[header::CONTENT_SECURITY_POLICY].to_str().unwrap().contains("default-src 'none'"));
    assert!(h.get(header::STRICT_TRANSPORT_SECURITY).is_none(), "HSTS off unless configured");
}

#[tokio::test]
async fn bench_endpoints_only_when_enabled() {
    let app = app_api::build_router(state_with(test_config(), vec![]));
    let res = send(&app, get_req("/bench/json")).await;
    assert_eq!(body_json(res).await["message"], "Hello, World!");
    let mut cfg = test_config();
    cfg.http.bench_endpoints = false;
    let app = app_api::build_router(state_with(cfg, vec![]));
    assert_eq!(send(&app, get_req("/bench/json")).await.status(), StatusCode::NOT_FOUND);
}

fn echo_router(state: app_api::AppState) -> Router {
    let api = Router::new().route("/api/v1/echo", post(|body: String| async move { body })).route(
        "/api/v1/slow",
        get(|| async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            "late"
        }),
    );
    app_api::router::build_router_with(state, api, Router::new())
}

#[tokio::test]
async fn body_limit_enforced() {
    let mut cfg = test_config();
    cfg.http.body_limit_bytes = 16;
    let app = echo_router(state_with(cfg, vec![]));
    let small = Request::post("/api/v1/echo").body(Body::from("tiny")).unwrap();
    assert_eq!(send(&app, small).await.status(), StatusCode::OK);
    let big = Request::post("/api/v1/echo").body(Body::from(vec![b'x'; 1024])).unwrap();
    assert_eq!(send(&app, big).await.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn request_timeout_returns_504() {
    let mut cfg = test_config();
    cfg.http.request_timeout_ms = 50;
    let app = echo_router(state_with(cfg, vec![]));
    let res = send(&app, get_req("/api/v1/slow")).await;
    assert_eq!(res.status(), StatusCode::GATEWAY_TIMEOUT);
}

#[tokio::test]
async fn cross_site_post_rejected_by_csrf_layer() {
    let app = echo_router(state_with(test_config(), vec![]));
    let req = Request::post("/api/v1/echo")
        .header("host", "localhost:8080")
        .header("sec-fetch-site", "cross-site")
        .header("origin", "https://evil.example")
        .body(Body::from("x"))
        .unwrap();
    assert_eq!(send(&app, req).await.status(), StatusCode::FORBIDDEN);
    let same = Request::post("/api/v1/echo")
        .header("host", "localhost:5173")
        .header("sec-fetch-site", "same-origin")
        .header("origin", "http://localhost:5173")
        .body(Body::from("x"))
        .unwrap();
    assert_eq!(send(&app, same).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn rate_limit_returns_429_with_retry_after() {
    let mut cfg = test_config();
    cfg.http.trust_forwarded_for = true;
    let state = app_api::AppState::builder(cfg, build_info())
        .rate_limiter(Arc::new(MemoryRateLimiter::new(Quota { per_second: 1.0, burst: 2 })))
        .build();
    let app = app_api::build_router(state);
    let req = || Request::get("/healthz").header("cf-connecting-ip", "9.9.9.9").body(Body::empty()).unwrap();
    assert_eq!(send(&app, req()).await.status(), StatusCode::OK);
    assert_eq!(send(&app, req()).await.status(), StatusCode::OK);
    let res = send(&app, req()).await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(res.headers().contains_key(header::RETRY_AFTER));
    let other = Request::get("/healthz").header("cf-connecting-ip", "8.8.8.8").body(Body::empty()).unwrap();
    assert_eq!(send(&app, other).await.status(), StatusCode::OK, "limits are per client");
}

#[tokio::test]
async fn load_is_shed_beyond_max_inflight() {
    let mut cfg = test_config();
    cfg.http.max_inflight = 1;
    let state = state_with(cfg, vec![]);
    let app = echo_router(state.clone());
    let held = state.inflight.clone().try_acquire_owned().unwrap(); // simulate one in-flight request
    let res = send(&app, get_req("/healthz")).await;
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(res.headers()[header::RETRY_AFTER], "1");
    drop(held);
    assert_eq!(send(&app, get_req("/healthz")).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn graceful_shutdown_drains_then_stops() {
    let mut cfg = test_config();
    cfg.http.shutdown_drain_ms = 200;
    let state = state_with(cfg, vec![]);
    let app = echo_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(app_api::server::serve(listener, app, state.clone()));
    let url = |p: &str| format!("http://{addr}{p}");
    let client = TestClient::new();
    assert_eq!(client.get_status(&url("/readyz")).await, 200);

    // Request shutdown the same way a signal would (cancel → drain → stop).
    let st = state.clone();
    tokio::spawn(async move {
        st.lifecycle.start_draining();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(client.get_status(&url("/readyz")).await, 503, "draining is visible to load balancers");
    state.lifecycle.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), server).await.expect("server stops").unwrap().unwrap();
}

/// Minimal HTTP/1.1 client over TCP to avoid a heavyweight dev-dependency here.
struct TestClient;
impl TestClient {
    fn new() -> Self {
        Self
    }
    async fn get_status(&self, url: &str) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let rest = url.strip_prefix("http://").unwrap();
        let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap();
        let mut s = tokio::net::TcpStream::connect(host).await.unwrap();
        s.write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.unwrap();
        let line = String::from_utf8_lossy(&buf);
        line.split_whitespace().nth(1).unwrap().parse().unwrap()
    }
}
