//! Router assembly and the middleware stack.
//!
//! Outermost → innermost for every request:
//! request-id (generate/accept) → trace span → panic catcher → sensitive-header redaction →
//! security headers → CORS → CSRF (Fetch Metadata / Origin) → client ip → metrics →
//! [API group] load shedding → authentication → rate limit (principal or IP) → timeout →
//! body limit → handler. Ops endpoints bypass shedding and rate limiting.
//!
//! Streaming routes (SSE/WebSocket) skip timeouts and load shedding; they end on shutdown.

use std::{path::Path, sync::Arc};

use app_errors::ApiError;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderName, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use tower::ServiceBuilder;
use tower_http::{
    catch_panic::CatchPanicLayer,
    compression::{
        CompressionLayer,
        predicate::{DefaultPredicate, Predicate, SizeAbove},
    },
    cors::{AllowOrigin, CorsLayer},
    csrf::CsrfLayer,
    limit::RequestBodyLimitLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    sensitive_headers::SetSensitiveHeadersLayer,
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
    trace::{DefaultOnFailure, TraceLayer},
};

use crate::{
    middleware as mw,
    routes::{bench, ops, realtime},
    state::AppState,
};

/// CSP for the HTML application shell. No inline scripts or styles; everything same-origin.
pub const APP_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; \
font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'";
/// CSP for API responses: nothing may be loaded or framed.
pub const API_CSP: &str = "default-src 'none'; frame-ancestors 'none'; base-uri 'none'";

/// The full application: identity, account, organisations, admin, realtime.
pub fn build_router(state: AppState) -> Router {
    let api = crate::routes::auth_routes().merge(crate::routes::api_routes(state.config.admin.enabled));
    build_router_with(state, api, crate::routes::stream_routes())
}

/// `api` routes get timeouts + load shedding; `streams` are long-lived. Both receive state.
pub fn build_router_with(state: AppState, api: Router<AppState>, streams: Router<AppState>) -> Router {
    let cfg = &state.config.http;

    // With an internal ops port, metrics and detailed readiness live only there (see ops_router).
    let internal_ops = cfg.ops_port.is_some();
    let mut ordinary = Router::new()
        .route("/healthz", get(ops::healthz))
        .route("/readyz", if internal_ops { get(ops::readyz_public) } else { get(ops::readyz) })
        .route("/version", get(ops::version))
        .merge(api)
        .route("/api/{*rest}", any(api_not_found));
    if state.config.telemetry.metrics && !internal_ops {
        ordinary = ordinary.route("/metrics", get(ops::metrics));
    } else {
        // Explicit 404: otherwise the SPA fallback would answer /metrics with index.html.
        ordinary = ordinary.route("/metrics", any(not_found));
    }
    if cfg.bench_endpoints {
        ordinary = ordinary
            .route("/bench/plaintext", get(bench::plaintext))
            .route("/bench/json", get(bench::json))
            .route("/bench/list", get(bench::list))
            .route("/bench/db", get(bench::db_read))
            .route("/bench/updates", get(bench::db_write))
            .route("/bench/cached", get(bench::cached_read));
    }
    let ordinary = ordinary.layer(
        ServiceBuilder::new()
            .layer(middleware::from_fn_with_state(state.clone(), mw::shed_load))
            .layer(middleware::from_fn_with_state(state.clone(), crate::auth::authenticate))
            // After authentication: signed-in principals get their own bucket (not their IP's).
            .layer(middleware::from_fn_with_state(state.clone(), mw::rate_limit))
            .layer(TimeoutLayer::with_status_code(StatusCode::GATEWAY_TIMEOUT, cfg.request_timeout()))
            .layer(DefaultBodyLimit::max(cfg.body_limit_bytes))
            .layer(RequestBodyLimitLayer::new(cfg.body_limit_bytes)),
    );

    // Authenticated streams get the auth layer; the public demo streams are added afterwards
    // (a layer applies only to routes that exist when it is added).
    let streams = streams
        .layer(middleware::from_fn_with_state(state.clone(), crate::auth::authenticate))
        .route("/events/public", get(realtime::public_sse))
        .route("/ws/public", get(realtime::public_ws));

    let mut app = Router::new().merge(ordinary).merge(streams);
    if let Some(dir) = cfg.static_dir.as_deref() {
        app = app.fallback_service(static_files(dir));
    } else {
        app = app.fallback(not_found);
    }

    if cfg.compression {
        app = app.layer(compression());
    }

    let sensitive: Arc<[HeaderName]> =
        Arc::new([header::AUTHORIZATION, header::COOKIE, header::SET_COOKIE, HeaderName::from_static("x-csrf-token")]);
    app.layer(
        ServiceBuilder::new()
            .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
            .layer(PropagateRequestIdLayer::x_request_id())
            // 5xx causes are logged once by ApiError (with the cause); the trace layer only records at DEBUG.
            .layer(
                TraceLayer::new_for_http()
                    .make_span_with(make_span)
                    .on_failure(DefaultOnFailure::new().level(tracing::Level::DEBUG)),
            )
            .layer(CatchPanicLayer::custom(|_| ApiError::internal("handler panicked").into_response()))
            .layer(SetSensitiveHeadersLayer::from_shared(sensitive))
            .layer(middleware::from_fn_with_state(state.clone(), security_headers))
            .layer(cors(&state))
            .layer(csrf(&state))
            .layer(middleware::from_fn_with_state(state.clone(), mw::client_ip))
            .layer(middleware::from_fn(mw::http_metrics)),
    )
    .with_state(state)
}

/// On-the-fly compression for API responses (tower-http defaults: brotli quality 4, gzip 6, about
/// 50 µs for a 13 KB list page). Skipped: responses under 1 KiB, event streams, images, anything
/// already encoded (the precompressed static files), and `Cache-Control: no-store` responses,
/// which carry credentials and must not share a compression context with reflected input (BREACH).
fn compression() -> CompressionLayer<impl Predicate> {
    let not_secret = |_: StatusCode, _: axum::http::Version, h: &axum::http::HeaderMap, _: &axum::http::Extensions| {
        !h.get(header::CACHE_CONTROL).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("no-store"))
    };
    CompressionLayer::new().compress_when(DefaultPredicate::new().and(SizeAbove::new(1024)).and(not_secret))
}

fn make_span(req: &Request) -> tracing::Span {
    let request_id = req.headers().get("x-request-id").and_then(|v| v.to_str().ok()).unwrap_or("-");
    let route = req.extensions().get::<axum::extract::MatchedPath>().map_or("unmatched", |p| p.as_str());
    let span = tracing::info_span!("http", method = %req.method(), route, request_id, trace_id = tracing::field::Empty);
    app_telemetry::propagation::set_parent_from_headers(&span, req.headers());
    if let Some(id) = app_telemetry::propagation::trace_id(&span) {
        span.record("trace_id", id);
    }
    span
}

fn cors(state: &AppState) -> CorsLayer {
    let origins: Vec<HeaderValue> =
        state.config.http.cors_origins.iter().filter_map(|o| HeaderValue::from_str(o).ok()).collect();
    if origins.is_empty() {
        // Same-origin only: no CORS headers are emitted at all.
        return CorsLayer::new();
    }
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE, HeaderName::from_static("x-csrf-token"), header::AUTHORIZATION])
}

fn csrf(state: &AppState) -> CsrfLayer {
    let mut layer = CsrfLayer::new();
    for origin in std::iter::once(&state.config.auth.public_origin).chain(&state.config.http.cors_origins) {
        match layer.clone().add_trusted_origin(origin.trim_end_matches('/')) {
            Ok(l) => layer = l,
            Err(e) => tracing::warn!(%origin, error = %e, "ignoring invalid trusted origin"),
        }
    }
    layer
}

/// Paths served by the API (everything else is the SPA or its assets).
fn is_api_path(path: &str) -> bool {
    path.starts_with("/api/")
        || path.starts_with("/auth/")
        || path.starts_with("/bench/")
        || matches!(path, "/healthz" | "/readyz" | "/version" | "/metrics")
}

async fn security_headers(State(state): State<AppState>, req: Request, next: Next) -> Response {
    // Choose the CSP by *request path*, not response content type: a 304 Not Modified for the
    // SPA has no Content-Type, and browsers merge 304 headers into the cached page, so a
    // content-type rule would attach the API policy to the app and block its own scripts.
    let api = is_api_path(req.uri().path());
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    let csp = if api { API_CSP } else { APP_CSP };
    h.entry(header::CONTENT_SECURITY_POLICY).or_insert(HeaderValue::from_static(csp));
    if api {
        // Per-user data: only the user's own browser may keep it, and it revalidates every time.
        // A CDN or proxy in front never stores it. Handlers may set their own policy, such as
        // `no-store` on responses carrying credentials.
        h.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static("private, no-cache"));
    }
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("strict-origin-when-cross-origin"));
    h.insert("cross-origin-opener-policy", HeaderValue::from_static("same-origin"));
    h.insert("cross-origin-resource-policy", HeaderValue::from_static("same-origin"));
    h.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=(), usb=()"),
    );
    if state.config.http.hsts {
        h.insert(header::STRICT_TRANSPORT_SECURITY, HeaderValue::from_static("max-age=63072000; includeSubDomains"));
    }
    res
}

/// Built frontend: hashed assets are immutable; everything else falls back to `index.html`
/// (client-side routing) and is revalidated on every load.
fn static_files(dir: &Path) -> Router {
    let index = dir.join("index.html");
    let assets = ServeDir::new(dir.join("assets")).precompressed_br().precompressed_gzip();
    let spa = ServeDir::new(dir).precompressed_br().precompressed_gzip().fallback(ServeFile::new(index));
    Router::new()
        .nest_service(
            "/assets",
            ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::overriding(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=31536000, immutable"),
                ))
                .service(assets),
        )
        .fallback_service(
            ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static("no-cache")))
                .service(spa),
        )
}

async fn api_not_found() -> ApiError {
    ApiError::NotFound
}

async fn not_found() -> ApiError {
    ApiError::NotFound
}

/// Internal operations listener (`http.ops_port`): liveness, detailed readiness, version and
/// metrics. No authentication, so bind it to a private interface or leave it unpublished.
pub fn ops_router(state: AppState) -> Router {
    let mut r = Router::new()
        .route("/healthz", get(ops::healthz))
        .route("/readyz", get(ops::readyz))
        .route("/version", get(ops::version));
    if state.config.telemetry.metrics {
        r = r.route("/metrics", get(ops::metrics));
    }
    r.with_state(state)
}
