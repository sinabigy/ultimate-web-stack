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
    let req = |path: &str, ip: &str| Request::get(path).header("cf-connecting-ip", ip).body(Body::empty()).unwrap();
    assert_eq!(send(&app, req("/api/v1/nope", "9.9.9.9")).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(send(&app, req("/api/v1/nope", "9.9.9.9")).await.status(), StatusCode::NOT_FOUND);
    let res = send(&app, req("/api/v1/nope", "9.9.9.9")).await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(res.headers().contains_key(header::RETRY_AFTER));
    assert_eq!(
        send(&app, req("/api/v1/nope", "8.8.8.8")).await.status(),
        StatusCode::NOT_FOUND,
        "limits are per client"
    );
    // The limited client's health checks and scrapes still pass.
    for path in ["/healthz", "/readyz", "/version"] {
        assert_eq!(send(&app, req(path, "9.9.9.9")).await.status(), StatusCode::OK, "{path} is never rate limited");
    }
}

#[tokio::test]
async fn load_is_shed_beyond_max_inflight() {
    let mut cfg = test_config();
    cfg.http.max_inflight = 1;
    let state = state_with(cfg, vec![]);
    let app = echo_router(state.clone());
    let held = state.inflight.clone().try_acquire_owned().unwrap(); // simulate one in-flight request
    let res = send(&app, get_req("/api/v1/missing")).await;
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(res.headers()[header::RETRY_AFTER], "1");
    assert_eq!(send(&app, get_req("/healthz")).await.status(), StatusCode::OK, "liveness is never shed");
    assert_eq!(send(&app, get_req("/readyz")).await.status(), StatusCode::OK, "readiness is never shed");
    drop(held);
    assert_eq!(send(&app, get_req("/api/v1/missing")).await.status(), StatusCode::NOT_FOUND);
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

#[tokio::test]
async fn spa_revalidation_304_keeps_app_csp() {
    let dir = std::env::temp_dir().join(format!("spa-{}", uuid_like()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html><script type=module src=/assets/a.js></script>").unwrap();
    let mut cfg = test_config();
    cfg.http.static_dir = Some(dir.clone());
    let app = app_api::build_router(state_with(cfg, vec![]));
    let first = send(&app, get_req("/account/sessions")).await;
    assert_eq!(first.status(), StatusCode::OK);
    let csp = first.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().to_string();
    assert!(csp.contains("script-src 'self'"), "{csp}");
    let validator = first.headers().get(header::LAST_MODIFIED).cloned().expect("static files carry Last-Modified");
    let again = send(
        &app,
        Request::get("/account/sessions").header(header::IF_MODIFIED_SINCE, validator).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(again.status(), StatusCode::NOT_MODIFIED, "revalidation path exercised");
    let csp = again
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    assert!(
        csp.contains("script-src 'self'") && !csp.contains("default-src 'none'"),
        "304 must keep the app CSP: {csp}"
    );
    // API paths keep the locked-down policy.
    let api = send(&app, get_req("/api/v1/nope")).await;
    assert!(api.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().contains("default-src 'none'"));
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn static_assets_use_build_time_compression() {
    // `npm run build` writes .br/.gz siblings (frontend/scripts/precompress.mjs); the server picks
    // one by Accept-Encoding, so no CPU is spent compressing per request.
    let dir = std::env::temp_dir().join(format!("spa-{}", uuid_like()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html>").unwrap();
    std::fs::write(dir.join("assets/a.js"), "console.log('identity')").unwrap();
    std::fs::write(dir.join("assets/a.js.br"), "brotli-bytes").unwrap();
    std::fs::write(dir.join("assets/a.js.gz"), "gzip-bytes").unwrap();
    let mut cfg = test_config();
    cfg.http.static_dir = Some(dir.clone());
    let app = app_api::build_router(state_with(cfg, vec![]));
    let fetch = |enc: Option<&'static str>| {
        let mut req = Request::get("/assets/a.js");
        if let Some(e) = enc {
            req = req.header(header::ACCEPT_ENCODING, e);
        }
        send(&app, req.body(Body::empty()).unwrap())
    };
    for (accept, encoding, body) in [
        (Some("gzip, deflate, br, zstd"), Some("br"), "brotli-bytes"),
        (Some("gzip"), Some("gzip"), "gzip-bytes"),
        (None, None, "console.log('identity')"),
    ] {
        let res = fetch(accept).await;
        assert_eq!(res.status(), StatusCode::OK);
        let h = res.headers().clone();
        assert_eq!(h.get(header::CONTENT_ENCODING).map(|v| v.to_str().unwrap()), encoding, "{accept:?}");
        assert_eq!(h[header::CONTENT_TYPE], "text/javascript", "the original type, not the variant's");
        assert!(
            h[header::VARY].to_str().unwrap().to_ascii_lowercase().contains("accept-encoding"),
            "caches key on encoding"
        );
        assert_eq!(h[header::CACHE_CONTROL], "public, max-age=31536000, immutable");
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(bytes, body.as_bytes(), "{accept:?}");
    }
    // A missing asset is a 404, never the SPA shell (a stale chunk must fail loudly).
    assert_eq!(fetch(None).await.status(), StatusCode::OK);
    let missing = send(&app, get_req("/assets/a.js.map")).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn api_responses_are_compressed_by_negotiation_except_secrets_and_streams() {
    let big = "x".repeat(4096);
    let api = Router::new()
        .route(
            "/api/big",
            get({
                let b = big.clone();
                move || async move { b }
            }),
        )
        .route("/api/small", get(|| async { "tiny" }))
        .route(
            "/api/secret",
            get({
                let b = big.clone();
                move || async move { ([(header::CACHE_CONTROL, "no-store")], b) }
            }),
        )
        .route(
            "/api/events",
            get({
                let b = big.clone();
                move || async move { ([(header::CONTENT_TYPE, "text/event-stream")], b) }
            }),
        );
    let call = |app: Router, path: &'static str, enc: Option<&'static str>| async move {
        let mut req = Request::get(path);
        if let Some(e) = enc {
            req = req.header(header::ACCEPT_ENCODING, e);
        }
        let res = send(&app, req.body(Body::empty()).unwrap()).await;
        let encoding = res.headers().get(header::CONTENT_ENCODING).map(|v| v.to_str().unwrap().to_string());
        let vary =
            res.headers().get(header::VARY).map(|v| v.to_str().unwrap().to_ascii_lowercase()).unwrap_or_default();
        let len = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap().len();
        (encoding, vary, len)
    };
    let app = app_api::router::build_router_with(state_with(test_config(), vec![]), api.clone(), Router::new());

    let (enc, vary, len) = call(app.clone(), "/api/big", Some("gzip, deflate, br, zstd")).await;
    assert_eq!(enc.as_deref(), Some("br"));
    assert!(vary.contains("accept-encoding"), "{vary}");
    assert!(len < 200, "4 KiB of repetition compresses: {len}");
    assert_eq!(call(app.clone(), "/api/big", Some("gzip")).await.0.as_deref(), Some("gzip"));
    let (enc, _, len) = call(app.clone(), "/api/big", None).await;
    assert_eq!((enc, len), (None, 4096), "no Accept-Encoding: identity");
    let plain = send(&app, get_req("/api/big")).await;
    assert_eq!(
        plain.headers().get(header::CONTENT_LENGTH).map(|v| v.to_str().unwrap()),
        Some("4096"),
        "identity keeps its length"
    );
    assert_eq!(call(app.clone(), "/api/small", Some("br")).await.0, None, "below 1 KiB: not worth it");
    assert_eq!(
        call(app.clone(), "/api/secret", Some("br")).await.0,
        None,
        "no-store responses stay uncompressed (BREACH)"
    );
    assert_eq!(
        call(app.clone(), "/api/events", Some("br")).await.0,
        None,
        "event streams are never buffered by a compressor"
    );

    let mut off = test_config();
    off.http.compression = false;
    let app = app_api::router::build_router_with(state_with(off, vec![]), api, Router::new());
    assert_eq!(call(app, "/api/big", Some("br")).await.0, None, "http.compression = false");
}

#[tokio::test]
async fn api_responses_are_private_and_handlers_keep_their_own_policy() {
    let api = Router::new()
        .route("/api/data", get(|| async { "per-user" }))
        .route("/api/secret", get(|| async { ([(header::CACHE_CONTROL, "no-store")], "token") }));
    let app = app_api::router::build_router_with(state_with(test_config(), vec![]), api, Router::new());
    let cc = |res: axum::response::Response| res.headers()[header::CACHE_CONTROL].to_str().unwrap().to_string();
    assert_eq!(cc(send(&app, get_req("/api/data")).await), "private, no-cache", "shared caches never store API data");
    assert_eq!(cc(send(&app, get_req("/api/secret")).await), "no-store");
    assert_eq!(cc(send(&app, get_req("/api/v1/missing")).await), "private, no-cache", "errors too");
}

fn uuid_like() -> String {
    format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
}

#[tokio::test]
async fn ops_port_keeps_metrics_and_check_details_off_the_public_port() {
    let mut cfg = test_config();
    cfg.http.ops_port = Some(9999);
    cfg.telemetry.metrics = true;
    let state = app_api::AppState::builder(cfg, build_info()).metrics(app_telemetry::local_metrics_handle()).build();
    let public = app_api::build_router(state.clone());
    let ops = app_api::router::ops_router(state);
    assert_eq!(
        send(&public, get_req("/metrics")).await.status(),
        StatusCode::NOT_FOUND,
        "no metrics on the public port"
    );
    let mut with_spa = test_config();
    with_spa.http.ops_port = Some(9999);
    let dir = std::env::temp_dir().join(format!("spa-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html>").unwrap();
    with_spa.http.static_dir = Some(dir);
    let spa = app_api::build_router(app_api::AppState::builder(with_spa, build_info()).build());
    assert_eq!(send(&spa, get_req("/metrics")).await.status(), StatusCode::NOT_FOUND, "not the SPA fallback either");
    let res = send(&public, get_req("/readyz")).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert!(body.get("checks").is_none() && body["status"].is_string(), "status only: {body}");
    assert_eq!(send(&ops, get_req("/metrics")).await.status(), StatusCode::OK);
    let body = body_json(send(&ops, get_req("/readyz")).await).await;
    assert!(body["checks"].is_array(), "ops port has the details: {body}");
}
