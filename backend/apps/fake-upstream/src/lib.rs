//! Simulated external API provider.
//!
//! Behaviour (all adjustable at runtime with `POST /__control`):
//! - **Capacity**: `capacity` concurrent requests are served at `base_latency_ms ± jitter`;
//!   beyond that, latency grows proportionally to the overload (queueing), and above
//!   `hard_limit` concurrent requests the server answers 503 immediately.
//! - **Rate limit**: token bucket of `rps`/`burst`; excess requests get 429 + `Retry-After`.
//! - **Faults**: `error_rate` (500), `slow_rate` (responds after `slow_ms`), `outage` (all 503).
//! - **Observability**: counts accepted TCP connections (to measure connection reuse), responses
//!   by class, distinct idempotency keys, and peak concurrency (`GET /__stats`).

use std::{
    collections::HashSet,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
    serve::ListenerExt,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Behaviour {
    pub capacity: u64,
    pub hard_limit: u64,
    pub base_latency_ms: u64,
    pub jitter_ms: u64,
    pub rps: f64,
    pub burst: f64,
    pub error_rate: f64,
    pub slow_rate: f64,
    pub slow_ms: u64,
    pub outage: bool,
    pub retry_after_secs: u64,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            capacity: 1_000_000,
            hard_limit: 10_000_000,
            base_latency_ms: 0,
            jitter_ms: 0,
            rps: 0.0,
            burst: 0.0,
            error_rate: 0.0,
            slow_rate: 0.0,
            slow_ms: 5_000,
            outage: false,
            retry_after_secs: 1,
        }
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Stats {
    pub connections: u64,
    pub requests: u64,
    pub ok: u64,
    pub rate_limited: u64,
    pub errors: u64,
    pub unavailable: u64,
    pub inflight: u64,
    pub peak_inflight: u64,
    pub distinct_idempotency_keys: u64,
}

struct Inner {
    behaviour: Mutex<Behaviour>,
    bucket: Mutex<(f64, Instant)>,
    connections: AtomicU64,
    requests: AtomicU64,
    ok: AtomicU64,
    rate_limited: AtomicU64,
    errors: AtomicU64,
    unavailable: AtomicU64,
    inflight: AtomicU64,
    peak: AtomicU64,
    keys: Mutex<HashSet<String>>,
}

#[derive(Clone)]
pub struct Upstream(Arc<Inner>);

impl Upstream {
    pub fn new(b: Behaviour) -> Self {
        let burst = b.burst.max(1.0);
        Self(Arc::new(Inner {
            behaviour: Mutex::new(b),
            bucket: Mutex::new((burst, Instant::now())),
            connections: AtomicU64::new(0),
            requests: AtomicU64::new(0),
            ok: AtomicU64::new(0),
            rate_limited: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            unavailable: AtomicU64::new(0),
            inflight: AtomicU64::new(0),
            peak: AtomicU64::new(0),
            keys: Mutex::new(HashSet::new()),
        }))
    }

    pub fn set(&self, b: Behaviour) {
        *self.0.bucket.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = (b.burst.max(1.0), Instant::now());
        *self.0.behaviour.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = b;
    }

    pub fn behaviour(&self) -> Behaviour {
        self.0.behaviour.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub fn stats(&self) -> Stats {
        let i = &self.0;
        Stats {
            connections: i.connections.load(Ordering::Relaxed),
            requests: i.requests.load(Ordering::Relaxed),
            ok: i.ok.load(Ordering::Relaxed),
            rate_limited: i.rate_limited.load(Ordering::Relaxed),
            errors: i.errors.load(Ordering::Relaxed),
            unavailable: i.unavailable.load(Ordering::Relaxed),
            inflight: i.inflight.load(Ordering::Relaxed),
            peak_inflight: i.peak.load(Ordering::Relaxed),
            distinct_idempotency_keys: i.keys.lock().map(|k| k.len() as u64).unwrap_or(0),
        }
    }

    fn take_token(&self, b: &Behaviour) -> bool {
        if b.rps <= 0.0 {
            return true;
        }
        let mut g = self.0.bucket.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let refill = now.duration_since(g.1).as_secs_f64() * b.rps;
        g.0 = (g.0 + refill).min(b.burst.max(1.0));
        g.1 = now;
        if g.0 >= 1.0 {
            g.0 -= 1.0;
            true
        } else {
            false
        }
    }
}

pub struct Running {
    pub addr: SocketAddr,
    pub upstream: Upstream,
    handle: tokio::task::JoinHandle<()>,
}

impl Running {
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub async fn start(bind: SocketAddr, b: Behaviour) -> anyhow::Result<Running> {
    let upstream = Upstream::new(b);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    let counter = upstream.clone();
    let listener = listener.tap_io(move |tcp| {
        let _ = tcp.set_nodelay(true);
        counter.0.connections.fetch_add(1, Ordering::Relaxed);
    });
    let app = Router::new()
        .route("/v1/echo", post(echo))
        .route("/healthz", get(|| async { "ok" }))
        .route("/__control", post(control).get(get_control))
        .route("/__stats", get(get_stats))
        .with_state(upstream.clone());
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(Running { addr, upstream, handle })
}

#[derive(Deserialize, Default)]
struct EchoBody {
    #[serde(default)]
    tokens: u64,
}

struct InflightGuard<'a>(&'a Inner);
impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, Ordering::Relaxed);
    }
}

async fn echo(State(u): State<Upstream>, headers: HeaderMap, body: Option<Json<EchoBody>>) -> Response {
    let i = &u.0;
    i.requests.fetch_add(1, Ordering::Relaxed);
    if let Some(k) = headers.get("idempotency-key").and_then(|v| v.to_str().ok())
        && let Ok(mut keys) = i.keys.lock()
    {
        keys.insert(k.to_string());
    }
    let b = u.behaviour();
    if b.outage {
        i.unavailable.fetch_add(1, Ordering::Relaxed);
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if !u.take_token(&b) {
        i.rate_limited.fetch_add(1, Ordering::Relaxed);
        let mut r = (StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({"error": "rate_limited"}))).into_response();
        if let Ok(v) = HeaderValue::from_str(&b.retry_after_secs.to_string()) {
            r.headers_mut().insert(header::RETRY_AFTER, v);
        }
        return r;
    }
    let now_inflight = i.inflight.fetch_add(1, Ordering::Relaxed) + 1;
    let _g = InflightGuard(i);
    i.peak.fetch_max(now_inflight, Ordering::Relaxed);
    if now_inflight > b.hard_limit {
        i.unavailable.fetch_add(1, Ordering::Relaxed);
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let (roll_err, roll_slow, jitter): (f64, f64, u64) = {
        let mut bytes = [0u8; 24];
        rand::fill(&mut bytes);
        let f = |o: usize| u64::from_le_bytes(bytes[o..o + 8].try_into().unwrap_or([0; 8])) as f64 / u64::MAX as f64;
        (f(0), f(8), if b.jitter_ms > 0 { (f(16) * b.jitter_ms as f64) as u64 } else { 0 })
    };
    // Queueing model: past capacity, each extra concurrent request adds proportional delay.
    let overload = now_inflight.saturating_sub(b.capacity) as f64 / b.capacity.max(1) as f64;
    let mut latency = b.base_latency_ms as f64 * (1.0 + overload) + jitter as f64;
    if roll_slow < b.slow_rate {
        latency = b.slow_ms as f64;
    }
    if latency > 0.0 {
        tokio::time::sleep(Duration::from_secs_f64(latency / 1000.0)).await;
    }
    if roll_err < b.error_rate {
        i.errors.fetch_add(1, Ordering::Relaxed);
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "upstream_failure"})))
            .into_response();
    }
    i.ok.fetch_add(1, Ordering::Relaxed);
    let tokens = body.map(|b| b.0.tokens).unwrap_or(0);
    Json(serde_json::json!({"ok": true, "tokens": tokens, "latency_ms": latency as u64})).into_response()
}

async fn control(State(u): State<Upstream>, Json(b): Json<Behaviour>) -> StatusCode {
    u.set(b);
    StatusCode::NO_CONTENT
}

async fn get_control(State(u): State<Upstream>) -> Json<Behaviour> {
    Json(u.behaviour())
}

async fn get_stats(State(u): State<Upstream>) -> Json<Stats> {
    Json(u.stats())
}
