//! Request middleware. Order (outermost first) is assembled in `router.rs`.

use std::{
    net::{IpAddr, SocketAddr},
    time::{Duration, Instant},
};

use app_errors::ApiError;
use app_rate_limit::Decision;
use axum::{
    extract::{ConnectInfo, MatchedPath, Request, State},
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::state::AppState;

/// The client address as determined server-side. Behind a trusted proxy/CDN this comes from
/// `CF-Connecting-IP` / the *last* `X-Forwarded-For` hop appended by our proxy; otherwise from the
/// socket. Never use it for authorization, only for rate limiting and audit metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientIp(pub Option<IpAddr>);

pub fn client_ip_from(headers: &HeaderMap, socket: Option<SocketAddr>, trust_forwarded: bool) -> Option<IpAddr> {
    if trust_forwarded {
        if let Some(ip) =
            headers.get("cf-connecting-ip").and_then(|v| v.to_str().ok()).and_then(|s| s.trim().parse().ok())
        {
            return Some(ip);
        }
        // The right-most entry was appended by our own trusted proxy; left entries are client-controlled.
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.rsplit(',').next())
            .and_then(|s| s.trim().parse().ok())
        {
            return Some(ip);
        }
    }
    socket.map(|s| s.ip())
}

pub async fn client_ip(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let socket = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let ip = client_ip_from(req.headers(), socket, state.config.http.trust_forwarded_for);
    req.extensions_mut().insert(ClientIp(ip));
    next.run(req).await
}

/// RED metrics per route template (never per raw path: unbounded cardinality).
pub async fn http_metrics(req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().as_str().to_owned();
    let route = req.extensions().get::<MatchedPath>().map_or_else(|| "unmatched".to_owned(), |p| p.as_str().to_owned());
    let gauge = metrics::gauge!("app_http_inflight_requests");
    gauge.increment(1.0);
    let res = next.run(req).await;
    gauge.decrement(1.0);
    let status = res.status().as_u16().to_string();
    metrics::counter!("app_http_requests_total", "method" => method.clone(), "route" => route.clone(), "status" => status)
        .increment(1);
    metrics::histogram!("app_http_request_duration_seconds", "method" => method, "route" => route)
        .record(start.elapsed().as_secs_f64());
    res
}

/// Load shedding: beyond `http.max_inflight` concurrent requests, reject immediately with 503
/// rather than queueing. Queueing past capacity only converts overload into timeouts for everyone.
pub async fn shed_load(State(state): State<AppState>, req: Request, next: Next) -> Response {
    match state.inflight.clone().try_acquire_owned() {
        Ok(_permit) => next.run(req).await,
        Err(_) => {
            metrics::counter!("app_http_shed_total").increment(1);
            let mut res = ApiError::Unavailable("server capacity").into_response();
            res.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
            res
        }
    }
}

/// Per-client rate limiting. Key: authenticated principal when present (set by auth
/// middleware as `RateLimitKey`), otherwise client IP.
#[derive(Debug, Clone)]
pub struct RateLimitKey(pub String);

pub async fn rate_limit(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(limiter) = state.rate_limiter.clone() else { return next.run(req).await };
    let key = req
        .extensions()
        .get::<RateLimitKey>()
        .map(|k| k.0.clone())
        .or_else(|| req.extensions().get::<ClientIp>().and_then(|c| c.0).map(|ip| format!("ip:{ip}")));
    let Some(key) = key else { return next.run(req).await };
    match limiter.check(&key).await {
        Ok(Decision::Allow { remaining }) => {
            let mut res = next.run(req).await;
            if let Ok(v) = HeaderValue::from_str(&remaining.to_string()) {
                res.headers_mut().insert("ratelimit-remaining", v);
            }
            res
        }
        Ok(Decision::Deny { retry_after }) => {
            metrics::counter!("app_rate_limited_total").increment(1);
            ApiError::RateLimited { retry_after_secs: retry_after.as_secs().max(1) }.into_response()
        }
        // Limiter backend failure (e.g. Redis down): availability over limiting, but visible.
        Err(e) => {
            metrics::counter!("app_rate_limiter_errors_total").increment(1);
            tracing::warn!(error = %e, "rate limiter unavailable; allowing request");
            next.run(req).await
        }
    }
}

/// Small helper for handlers that need a deadline shorter than the global timeout.
pub async fn with_deadline<T>(d: Duration, f: impl std::future::Future<Output = T>) -> Result<T, ApiError> {
    tokio::time::timeout(d, f).await.map_err(|_| ApiError::Timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_ip_only_when_trusted_and_rightmost_hop() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_static("6.6.6.6, 10.0.0.1"));
        let sock: SocketAddr = "127.0.0.1:5000".parse().expect("addr");
        assert_eq!(client_ip_from(&h, Some(sock), false), Some(sock.ip()), "untrusted: socket");
        assert_eq!(client_ip_from(&h, Some(sock), true), "10.0.0.1".parse().ok(), "trusted: rightmost");
        h.insert("cf-connecting-ip", HeaderValue::from_static("1.2.3.4"));
        assert_eq!(client_ip_from(&h, Some(sock), true), "1.2.3.4".parse().ok());
    }
}
