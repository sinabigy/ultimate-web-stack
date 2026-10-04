//! Router assembly and the middleware stack.
//!
//! Outermost → innermost for every request:
//! request-id (generate/accept) → trace span → panic catcher → sensitive-header redaction →
//! security headers → CORS → CSRF (Fetch Metadata / Origin) → client ip → metrics →
//! rate limit → [per group] load shedding, timeout, body limit → handler.
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

    let mut ordinary = Router::new()
        .route("/healthz", get(ops::healthz))
        .route("/readyz", get(ops::readyz))
        .route("/version", get(ops::version))
        .merge(api)
        .route("/api/{*rest}", any(api_not_found));
    if state.config.telemetry.metrics {
        ordinary = ordinary.route("/metrics", get(ops::metrics));
    }
    if cfg.bench_endpoints {
        ordinary = ordinary.route("/bench/plaintext", get(bench::plaintext)).route("/bench/json", get(bench::json));
    }
    let ordinary = ordinary.layer(
        ServiceBuilder::new()
            .layer(middleware::from_fn_with_state(state.clone(), mw::shed_load))
            .layer(middleware::from_fn_with_state(state.clone(), crate::auth::authenticate))
            .layer(TimeoutLayer::with_status_code(StatusCode::GATEWAY_TIMEOUT, cfg.request_timeout()))
            .layer(DefaultBodyLimit::max(cfg.body_limit_bytes))
            .layer(RequestBodyLimitLayer::new(cfg.body_limit_bytes)),
    );

    let streams =
        streams.route("/events/public", get(realtime::public_sse)).route("/ws/public", get(realtime::public_ws));

    let mut app = Router::new().merge(ordinary).merge(streams);
    if let Some(dir) = cfg.static_dir.as_deref() {
        app = app.fallback_service(static_files(dir));
    } else {
        app = app.fallback(not_found);
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
            .layer(middleware::from_fn(mw::http_metrics))
            .layer(middleware::from_fn_with_state(state.clone(), mw::rate_limit)),
    )
    .with_state(state)
}

fn make_span(req: &Request) -> tracing::Span {
    let request_id = req.headers().get("x-request-id").and_then(|v| v.to_str().ok()).unwrap_or("-");
    let route = req.extensions().get::<axum::extract::MatchedPath>().map_or("unmatched", |p| p.as_str());
    tracing::info_span!("http", method = %req.method(), route, request_id, status = tracing::field::Empty)
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

async fn security_headers(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let is_html = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/html"));
    let h = res.headers_mut();
    let csp = if is_html { APP_CSP } else { API_CSP };
    h.entry(header::CONTENT_SECURITY_POLICY).or_insert(HeaderValue::from_static(csp));
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
