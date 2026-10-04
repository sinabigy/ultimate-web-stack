//! One external provider: pooled HTTP client + budgets + adaptive concurrency + breaker + retries.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use http::{HeaderValue, Method, StatusCode, header};

use crate::{
    adaptive::{AcquireError, AdaptiveConfig, AdaptiveLimiter, Outcome},
    breaker::{BreakerConfig, CircuitBreaker, Health},
    rate::RateLimiter,
    retry::{RetryBudget, RetryPolicy},
    stats::{HealthSnapshot, Kind, Stats},
};

#[derive(Debug, Clone)]
pub struct ProviderSettings {
    pub base_url: String,
    pub api_key: Option<String>,
    pub adaptive: AdaptiveConfig,
    pub requests_per_second: f64,
    pub burst: u32,
    pub tokens_per_minute: u64,
    pub request_timeout: Duration,
    pub connect_timeout: Duration,
    pub retry: RetryPolicy,
    pub breaker: BreakerConfig,
    pub http2_prior_knowledge: bool,
    pub pool_idle_timeout: Duration,
    pub dns_cache: bool,
}

impl ProviderSettings {
    pub fn from_definition(d: &app_config::ProviderDefinition) -> Self {
        Self {
            base_url: d.base_url.trim_end_matches('/').to_string(),
            api_key: (!d.api_key.is_empty()).then(|| d.api_key.expose().to_string()),
            adaptive: AdaptiveConfig {
                min: d.min_concurrency,
                max: d.max_concurrency,
                initial: d.initial_concurrency,
                ..Default::default()
            },
            requests_per_second: d.requests_per_second,
            burst: d.burst,
            tokens_per_minute: d.tokens_per_minute,
            request_timeout: Duration::from_millis(d.request_timeout_ms),
            connect_timeout: Duration::from_millis(d.connect_timeout_ms),
            retry: RetryPolicy { max_retries: d.max_retries, ..Default::default() },
            breaker: BreakerConfig::default(),
            http2_prior_knowledge: d.http2_prior_knowledge,
            pool_idle_timeout: Duration::from_secs(90),
            dns_cache: true,
        }
    }
}

/// Request priority: higher values get concurrency slots first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Priority(pub u8);

impl Priority {
    pub const BACKGROUND: Priority = Priority(10);
    pub const NORMAL: Priority = Priority(50);
    pub const INTERACTIVE: Priority = Priority(90);
}

#[derive(Debug, Clone)]
pub struct CallRequest {
    pub method: Method,
    /// Path relative to the provider base URL (e.g. "/v1/echo"). Never a full URL: the base URL
    /// comes from configuration, which keeps callers from steering requests elsewhere (SSRF).
    pub path: String,
    pub body: Option<serde_json::Value>,
    pub priority: Priority,
    /// Sent as `Idempotency-Key` on every attempt; required for retrying non-idempotent methods.
    pub idempotency_key: Option<String>,
    /// Estimated token cost for token-per-minute budgets (0 = not token-metered).
    pub estimated_tokens: u64,
    /// Total time budget across all attempts, waits and retries.
    pub deadline: Duration,
}

impl CallRequest {
    pub fn post(path: &str, body: serde_json::Value) -> Self {
        Self {
            method: Method::POST,
            path: path.to_string(),
            body: Some(body),
            priority: Priority::NORMAL,
            idempotency_key: None,
            estimated_tokens: 0,
            deadline: Duration::from_secs(30),
        }
    }
    pub fn idempotency_key(mut self, k: impl Into<String>) -> Self {
        self.idempotency_key = Some(k.into());
        self
    }
    pub fn priority(mut self, p: Priority) -> Self {
        self.priority = p;
        self
    }
    pub fn deadline(mut self, d: Duration) -> Self {
        self.deadline = d;
        self
    }
    pub fn tokens(mut self, t: u64) -> Self {
        self.estimated_tokens = t;
        self
    }
}

#[derive(Debug, Clone)]
pub struct CallResponse {
    pub status: StatusCode,
    pub body: Bytes,
    pub attempts: u32,
    pub latency: Duration,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum CallError {
    #[error("circuit open: provider is failing, call rejected without sending")]
    CircuitOpen,
    #[error("provider concurrency queue full (backpressure)")]
    QueueFull,
    #[error("deadline exceeded before the request could be sent")]
    DeadlineExceeded,
    #[error("request timed out")]
    Timeout,
    #[error("provider returned {status}")]
    Status { status: StatusCode, attempts: u32 },
    #[error("transport error: {0}")]
    Transport(String),
    #[error("invalid request: {0}")]
    Invalid(String),
}

impl CallError {
    /// Worth retrying later (job-level), as opposed to a request that can never succeed.
    pub fn is_transient(&self) -> bool {
        match self {
            CallError::Status { status, .. } => status.is_server_error() || *status == StatusCode::TOO_MANY_REQUESTS,
            CallError::Invalid(_) => false,
            _ => true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("cannot build provider client: {0}")]
pub struct BuildError(String);

pub struct Provider {
    name: String,
    settings: ProviderSettings,
    client: reqwest::Client,
    limiter: Arc<AdaptiveLimiter>,
    rate: RateLimiter,
    tokens: RateLimiter,
    breaker: CircuitBreaker,
    budget: RetryBudget,
    stats: Stats,
    /// Adaptive request-rate control (learned from 429s), capped by the configured contract rate.
    rate_ctl: std::sync::Mutex<RateCtl>,
}

#[derive(Debug, Clone, Copy)]
struct RateCtl {
    last_429: Option<Instant>,
    /// Growth is credited only for time after this instant (never for time spent paused).
    last_increase: Instant,
    /// When the measurement window for "what the provider accepts" starts (after a pause).
    window_start: Instant,
    /// Cap that was in force at the last 429 event: the provider's limit is at or below it.
    ceiling: Option<f64>,
    ceiling_at: Instant,
}

/// Rate-AIMD tuning. Every 429 event costs a provider-directed pause (`Retry-After`), which is
/// far more expensive than running slightly below the limit, so after learning a ceiling the
/// controller holds just below it and only probes upward slowly.
const RATE_BACKOFF: f64 = 0.85;
const RATE_HOLD_FRACTION: f64 = 0.95;
const RATE_HOLD_FOR: Duration = Duration::from_secs(30);
const RATE_PROBE_PER_SEC: f64 = 0.01;
/// Growth per 100ms below the hold level / with no ceiling learned yet (slow start).
const RATE_GROWTH: f64 = 1.05;
const RATE_GROWTH_UNKNOWN: f64 = 1.15;
/// Minimum un-paused time to trust an accepted-rate measurement.
const RATE_MIN_SAMPLE: Duration = Duration::from_millis(250);

impl Provider {
    pub fn new(name: &str, settings: ProviderSettings) -> Result<Self, BuildError> {
        let max_idle = settings.adaptive.max.max(1);
        let mut b = reqwest::Client::builder()
            // Persistent connections: keep enough idle sockets for the concurrency ceiling.
            .pool_max_idle_per_host(max_idle)
            .pool_idle_timeout(settings.pool_idle_timeout)
            .tcp_keepalive(Duration::from_secs(30))
            .tcp_nodelay(true)
            .connect_timeout(settings.connect_timeout)
            // HTTP/2 negotiated by ALPN on TLS; adaptive flow-control windows for throughput.
            .http2_adaptive_window(true)
            .http2_keep_alive_interval(Duration::from_secs(30))
            // Provider APIs must not redirect us to arbitrary hosts.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("app-networking/", env!("CARGO_PKG_VERSION")));
        if settings.http2_prior_knowledge {
            b = b.http2_prior_knowledge();
        }
        if settings.dns_cache {
            // In-process async resolver with TTL-respecting cache (no blocking getaddrinfo).
            b = b.hickory_dns(true);
        }
        let client = b.build().map_err(|e| BuildError(e.to_string()))?;
        let tpm = settings.tokens_per_minute as f64;
        Ok(Self {
            name: name.to_string(),
            limiter: AdaptiveLimiter::new(settings.adaptive),
            rate: RateLimiter::new(settings.requests_per_second, f64::from(settings.burst.max(1))),
            tokens: if tpm > 0.0 { RateLimiter::new(tpm / 60.0, tpm) } else { RateLimiter::unlimited() },
            breaker: CircuitBreaker::new(settings.breaker),
            budget: RetryBudget::new(settings.retry),
            stats: Stats::new(Duration::from_secs(30)),
            rate_ctl: std::sync::Mutex::new({
                let now = Instant::now();
                RateCtl { last_429: None, last_increase: now, window_start: now, ceiling: None, ceiling_at: now }
            }),
            client,
            settings,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn limiter(&self) -> &Arc<AdaptiveLimiter> {
        &self.limiter
    }

    pub fn health(&self) -> HealthSnapshot {
        let (n, ok, r429, r5xx, rto, p50, p95, p99, rps) = self.stats.summary();
        HealthSnapshot {
            provider: self.name.clone(),
            concurrency_limit: self.limiter.limit(),
            inflight: self.limiter.inflight(),
            queued: self.limiter.queued(),
            circuit: self.breaker.state().as_str(),
            window_secs: self.stats.window().as_secs(),
            requests: n,
            success_rate: ok,
            rate_429: r429,
            rate_5xx: r5xx,
            rate_timeout: rto,
            p50_ms: p50,
            p95_ms: p95,
            p99_ms: p99,
            throughput_rps: rps,
            paused_for_ms: self
                .rate
                .paused_until()
                .map_or(0, |p| p.saturating_duration_since(Instant::now()).as_millis() as u64),
            rate_cap_rps: self.rate.rate(),
        }
    }

    /// Rate-AIMD, one decrease per 429 *event* (debounced to 500ms).
    ///
    /// On an event the provider's limit is estimated as the rate it actually accepted since
    /// the last pause ended (successes / un-paused time, needs ≥250ms of data). The ceiling is
    /// min(current cap, that estimate) and the cap drops to 85% of it. While calm, the cap grows
    /// ×1.05 per 100ms of *un-paused* time up to 95% of the ceiling, holds there for 30s, then
    /// probes upward at 1% of the ceiling per second (limits get raised). With no ceiling yet,
    /// or once 1.5× above it, growth is ×1.15 per 100ms (slow start). Never above the
    /// configured contract rate. Measured in `benchmarks/` (rate_limited scenario).
    fn on_rate_limited(&self, pause_end: Instant) {
        let now = Instant::now();
        let Ok(mut c) = self.rate_ctl.lock() else { return };
        let fresh_event = c.last_429.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(500));
        c.last_429 = Some(now);
        c.last_increase = c.last_increase.max(pause_end);
        if !fresh_event {
            return;
        }
        // Most recent ≤500ms: while ramping, older samples were taken below the limit and would
        // bias the estimate low (measured: 87% of the true limit with a 1s window).
        let span = now.saturating_duration_since(c.window_start).min(Duration::from_millis(500));
        let accepted = (span >= RATE_MIN_SAMPLE)
            .then(|| self.stats.successes_within(span) as f64 / span.as_secs_f64())
            .filter(|r| *r >= 1.0);
        c.window_start = pause_end;
        let mut target = match (self.rate.rate(), accepted) {
            // Reliable: a ceiling to hold below.
            (cap, Some(acc)) => {
                let ceiling = cap.map_or(acc, |cap| cap.min(acc));
                c.ceiling = Some(ceiling);
                c.ceiling_at = now;
                ceiling * RATE_BACKOFF
            }
            (Some(cap), None) => cap * RATE_BACKOFF,
            // Too little data (e.g. the first burst at startup measures the provider's burst
            // allowance, not its rate): restart low and let fast growth find the limit.
            (None, None) => self.stats.successes_within(Duration::from_secs(1)).max(1) as f64,
        }
        .max(1.0);
        if self.settings.requests_per_second > 0.0 {
            target = target.min(self.settings.requests_per_second);
        }
        self.rate.set_rate(target, (target * 0.05).max(1.0));
        metrics::gauge!("app_provider_learned_rps", "provider" => self.name.clone()).set(target);
    }

    fn on_success_rate(&self) {
        let Some(current) = self.rate.rate() else { return };
        let Ok(mut c) = self.rate_ctl.lock() else { return };
        let now = Instant::now();
        let calm = c.last_429.is_none_or(|t| now.duration_since(t) > Duration::from_millis(500));
        let since = now.saturating_duration_since(c.last_increase);
        if !calm || since < Duration::from_millis(100) {
            return;
        }
        c.last_increase = now;
        // Grow only while the cap is actually the constraint (≥70% used over the last 500ms);
        // growing an unused cap teaches nothing and ends in a burst far above the limit.
        if (self.stats.successes_within(Duration::from_millis(500)) as f64) * 2.0 < current * 0.7 {
            return;
        }
        // Time-based (up to 2s worth per update): recovery speed does not depend on traffic.
        let steps = (since.as_secs_f64() / 0.1).min(20.0);
        let next = match c.ceiling {
            Some(ceiling) if current > ceiling * 1.5 => {
                c.ceiling = None;
                current * RATE_GROWTH_UNKNOWN.powf(steps)
            }
            Some(ceiling) => {
                let hold = ceiling * RATE_HOLD_FRACTION;
                if current < hold {
                    (current * RATE_GROWTH.powf(steps)).min(hold)
                } else if now.duration_since(c.ceiling_at) < RATE_HOLD_FOR {
                    current
                } else {
                    current + ceiling * RATE_PROBE_PER_SEC * since.as_secs_f64().min(2.0)
                }
            }
            None => current * RATE_GROWTH_UNKNOWN.powf(steps),
        };
        let configured = self.settings.requests_per_second;
        if configured > 0.0 && next >= configured {
            self.rate.set_rate(configured, f64::from(self.settings.burst.max(1)));
        } else if next != current {
            self.rate.set_rate(next, (next * 0.05).max(1.0));
        }
    }

    fn retryable_method(&self, req: &CallRequest) -> bool {
        matches!(req.method, Method::GET | Method::HEAD | Method::PUT | Method::DELETE | Method::OPTIONS)
            || req.idempotency_key.is_some()
    }

    #[tracing::instrument(name = "provider.call", skip_all, fields(provider = %self.name, path = %req.path, attempts = tracing::field::Empty))]
    pub async fn call(&self, req: CallRequest) -> Result<CallResponse, CallError> {
        if !req.path.starts_with('/')
            || req.path.starts_with("//")
            || req.path.contains("://")
            || req.path.contains("..")
        {
            return Err(CallError::Invalid("path must be relative to the provider base URL".into()));
        }
        let start = Instant::now();
        let deadline = start + req.deadline;
        let can_retry = self.retryable_method(&req);
        let mut attempt: u32 = 0;
        self.budget.record_attempt();
        loop {
            attempt += 1;
            tracing::Span::current().record("attempts", attempt);
            let result = self.attempt(&req, deadline).await;
            let (err, retry_after) = match result {
                Ok(resp) => {
                    metrics::counter!("app_provider_requests_total", "provider" => self.name.clone(), "outcome" => "ok").increment(1);
                    return Ok(CallResponse { attempts: attempt, latency: start.elapsed(), ..resp });
                }
                Err((e, ra)) => (e, ra),
            };
            let outcome_label = match &err {
                CallError::Status { status, .. } if *status == StatusCode::TOO_MANY_REQUESTS => "rate_limited",
                CallError::Status { status, .. } if status.is_server_error() => "server_error",
                CallError::Status { .. } => "client_error",
                CallError::Timeout => "timeout",
                CallError::CircuitOpen => "circuit_open",
                CallError::QueueFull => "queue_full",
                CallError::DeadlineExceeded => "deadline",
                CallError::Transport(_) => "transport",
                CallError::Invalid(_) => "invalid",
            };
            metrics::counter!("app_provider_requests_total", "provider" => self.name.clone(), "outcome" => outcome_label).increment(1);
            let retryable_error = matches!(&err, CallError::Timeout | CallError::Transport(_))
                || matches!(&err, CallError::Status { status, .. } if status.is_server_error() || *status == StatusCode::TOO_MANY_REQUESTS || *status == StatusCode::REQUEST_TIMEOUT);
            if !(retryable_error && can_retry) || attempt > self.settings.retry.max_retries {
                return Err(err);
            }
            let wait = self.settings.retry.backoff(attempt, retry_after);
            // Waiting exactly as the provider asked (Retry-After) is not a retry storm, so it
            // does not spend the retry budget; all other retries do.
            let provider_directed = retry_after.is_some();
            if Instant::now() + wait >= deadline || (!provider_directed && !self.budget.try_spend()) {
                return Err(err);
            }
            metrics::counter!("app_provider_retries_total", "provider" => self.name.clone()).increment(1);
            tokio::time::sleep(wait).await;
        }
    }

    /// One attempt. Errors carry an optional `Retry-After` hint.
    async fn attempt(
        &self,
        req: &CallRequest,
        deadline: Instant,
    ) -> Result<CallResponse, (CallError, Option<Duration>)> {
        if Instant::now() >= deadline {
            return Err((CallError::DeadlineExceeded, None));
        }
        if !self.breaker.allow() {
            return Err((CallError::CircuitOpen, None));
        }
        let gate = async {
            self.rate.acquire(1.0, deadline).await.map_err(|_| CallError::DeadlineExceeded)?;
            if req.estimated_tokens > 0 {
                self.tokens
                    .acquire(req.estimated_tokens as f64, deadline)
                    .await
                    .map_err(|_| CallError::DeadlineExceeded)?;
            }
            self.limiter.acquire(req.priority.0, deadline).await.map_err(|e| match e {
                AcquireError::QueueFull => CallError::QueueFull,
                AcquireError::Timeout => CallError::DeadlineExceeded,
            })
        };
        let permit = match gate.await {
            Ok(p) => p,
            Err(e) => {
                // Not sent: does not count against the breaker. A half-open probe slot taken by
                // `allow()` is returned by recording neither success nor failure here.
                return Err((e, None));
            }
        };
        metrics::gauge!("app_provider_concurrency_limit", "provider" => self.name.clone())
            .set(self.limiter.limit() as f64);
        metrics::gauge!("app_provider_queue_depth", "provider" => self.name.clone()).set(self.limiter.queued() as f64);

        let url = format!("{}{}", self.settings.base_url, req.path);
        let mut rb = self.client.request(req.method.clone(), &url).timeout(
            self.settings
                .request_timeout
                .min(deadline.saturating_duration_since(Instant::now()))
                .max(Duration::from_millis(1)),
        );
        if let Some(k) = &self.settings.api_key {
            rb = rb.bearer_auth(k);
        }
        if let Some(k) = &req.idempotency_key {
            rb = rb.header("idempotency-key", k);
        }
        if let Some(b) = &req.body {
            rb = rb.json(b);
        }
        let sent = Instant::now();
        let result = rb.send().await;
        let latency = sent.elapsed();
        let finish = |kind: Kind, outcome: Outcome, health: Option<Health>| {
            self.stats.record(kind, latency);
            if let Some(h) = health {
                self.breaker.record(h);
            }
            metrics::histogram!("app_provider_latency_seconds", "provider" => self.name.clone())
                .record(latency.as_secs_f64());
            outcome
        };
        match result {
            Ok(resp) => {
                let status = resp.status();
                let retry_after = resp.headers().get(header::RETRY_AFTER).and_then(parse_retry_after);
                let body = match resp.bytes().await {
                    Ok(b) => b,
                    Err(e) => {
                        permit.finish(finish(Kind::Transport, Outcome::Error, Some(Health::Hard)));
                        return Err((CallError::Transport(e.to_string()), None));
                    }
                };
                if status.is_success() {
                    permit.finish(finish(Kind::Ok, Outcome::Success, Some(Health::Success)));
                    self.on_success_rate();
                    return Ok(CallResponse { status, body, attempts: 1, latency });
                }
                let (kind, outcome, breaker) = match status {
                    // A rate limit is not a concurrency signal: the rate controller handles it.
                    // Shrinking concurrency too would compound the backoff (observed: limit → 1).
                    StatusCode::TOO_MANY_REQUESTS => (Kind::RateLimited, Outcome::Error, None),
                    StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT => {
                        (Kind::ServerError, Outcome::Overload, Some(Health::Soft))
                    }
                    s if s.is_server_error() => (Kind::ServerError, Outcome::Error, Some(Health::Hard)),
                    _ => (Kind::ClientError, Outcome::Error, Some(Health::Success)), // 4xx: caller's problem, provider healthy
                };
                permit.finish(finish(kind, outcome, breaker));
                if status == StatusCode::TOO_MANY_REQUESTS {
                    // Everyone waits: hammering a rate-limited provider only earns more 429s.
                    let pause = retry_after.unwrap_or(Duration::from_millis(500));
                    self.rate.pause_until(Instant::now() + pause);
                    self.on_rate_limited(Instant::now() + pause);
                }
                Err((CallError::Status { status, attempts: 1 }, retry_after))
            }
            Err(e) if e.is_timeout() => {
                permit.finish(finish(Kind::Timeout, Outcome::Overload, Some(Health::Hard)));
                Err((CallError::Timeout, None))
            }
            Err(e) => {
                permit.finish(finish(Kind::Transport, Outcome::Error, Some(Health::Hard)));
                Err((CallError::Transport(error_chain(&e)), None))
            }
        }
    }
}

fn parse_retry_after(v: &HeaderValue) -> Option<Duration> {
    v.to_str().ok()?.trim().parse::<u64>().ok().map(|s| Duration::from_secs(s.min(3600)))
}

fn error_chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        s.push_str(": ");
        s.push_str(&c.to_string());
        cur = c.source();
    }
    s
}
