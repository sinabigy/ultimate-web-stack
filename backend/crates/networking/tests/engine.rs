#![allow(clippy::unwrap_used)]
//! Outbound engine against a simulated provider over real TCP.

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use app_networking::{
    AdaptiveConfig, CallError, CallRequest, Provider, ProviderSettings, breaker::BreakerConfig, retry::RetryPolicy,
};
use fake_upstream::{Behaviour, start};
use futures_util::stream::{self, StreamExt};
use serde_json::json;

fn settings(url: &str) -> ProviderSettings {
    ProviderSettings {
        base_url: url.to_string(),
        api_key: None,
        adaptive: AdaptiveConfig { min: 1, max: 256, initial: 32, ..Default::default() },
        requests_per_second: 0.0,
        burst: 0,
        tokens_per_minute: 0,
        request_timeout: Duration::from_secs(5),
        connect_timeout: Duration::from_secs(2),
        retry: RetryPolicy { max_retries: 6, base: Duration::from_millis(20), ..Default::default() },
        breaker: BreakerConfig::default(),
        http2_prior_knowledge: false,
        pool_idle_timeout: Duration::from_secs(90),
        dns_cache: false,
    }
}

async fn upstream(b: Behaviour) -> fake_upstream::Running {
    start(SocketAddr::from(([127, 0, 0, 1], 0)), b).await.unwrap()
}

async fn fire(p: &Arc<Provider>, n: usize, parallel: usize, keyed: bool) -> (usize, usize, Vec<CallError>) {
    let results: Vec<_> = stream::iter(0..n)
        .map(|i| {
            let p = p.clone();
            async move {
                let mut r = CallRequest::post("/v1/echo", json!({"tokens": 1})).deadline(Duration::from_secs(30));
                if keyed {
                    r = r.idempotency_key(format!("k-{i}"));
                }
                p.call(r).await
            }
        })
        .buffer_unordered(parallel)
        .collect()
        .await;
    let ok = results.iter().filter(|r| r.is_ok()).count();
    let errs: Vec<CallError> = results.into_iter().filter_map(Result::err).collect();
    (ok, n - ok, errs)
}

#[tokio::test]
async fn persistent_connections_are_reused() {
    let up = upstream(Behaviour { base_latency_ms: 1, ..Default::default() }).await;
    let p = Arc::new(Provider::new("t", settings(&up.url())).unwrap());
    let (ok, failed, _) = fire(&p, 2000, 32, false).await;
    assert_eq!((ok, failed), (2000, 0));
    let s = up.upstream.stats();
    let reuse = 1.0 - s.connections as f64 / s.requests as f64;
    assert!(s.connections <= 40, "connections={} for 2000 requests at concurrency 32", s.connections);
    assert!(reuse > 0.98, "connection reuse {reuse:.3}");
}

#[tokio::test]
async fn rate_limited_provider_is_respected_and_work_completes() {
    // Provider allows 300 req/s (burst 30) and answers 429 + Retry-After: 1 beyond that.
    let up = upstream(Behaviour { rps: 300.0, burst: 30.0, retry_after_secs: 1, ..Default::default() }).await;
    let p = Arc::new(Provider::new("t", settings(&up.url())).unwrap());
    let start = Instant::now();
    let (ok, failed, errs) = fire(&p, 600, 64, true).await;
    let s = up.upstream.stats();
    assert_eq!(failed, 0, "every request eventually succeeds: {:?}", errs.first());
    assert_eq!(ok, 600);
    // Useful throughput, not raw rate: the engine pauses on Retry-After instead of hammering.
    let waste = s.rate_limited as f64 / s.requests as f64;
    assert!(waste < 0.35, "429s were {:.0}% of requests sent ({} of {})", waste * 100.0, s.rate_limited, s.requests);
    assert_eq!(s.distinct_idempotency_keys, 600, "retries reuse the same idempotency key");
    // About 5 s normally (2 s ideal + learning pauses); the bound only catches a stuck controller.
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(20),
        "took {elapsed:?}: {} 429s of {} requests, rate cap {:?}",
        s.rate_limited,
        s.requests,
        p.health().rate_cap_rps
    );
}

#[tokio::test]
async fn learned_rate_converges_near_the_provider_limit() {
    // Regression test for the rate controller (benchmarks: 163 → ~500 useful req/s at a 500/s
    // limit). After learning, sustained throughput must stay close to the limit, without
    // collapsing the concurrency limit (a 429 is a rate signal, not a concurrency signal).
    let limit = 400.0;
    let up = upstream(Behaviour { rps: limit, burst: 40.0, retry_after_secs: 1, ..Default::default() }).await;
    let p = Arc::new(Provider::new("t", settings(&up.url())).unwrap());
    // Learning: each 429 costs a 1s pause; the estimate includes the provider's burst, so the
    // hold level converges over a few events (measured: 3-4 at this limit).
    let (ok, failed, _) = fire(&p, 3200, 64, true).await;
    assert_eq!((ok, failed), (3200, 0));
    let before = up.upstream.stats();
    let start = Instant::now();
    let (ok, failed, _) = fire(&p, 1600, 64, true).await; // steady state
    let elapsed = start.elapsed().as_secs_f64();
    let after = up.upstream.stats();
    assert_eq!((ok, failed), (1600, 0));
    let rate = 1600.0 / elapsed;
    let new_429 = after.rate_limited - before.rate_limited;
    let cap = p.health().rate_cap_rps;
    assert!(rate >= 0.85 * limit, "steady useful rate {rate:.0}/s < 85% of {limit}/s (429s: {new_429}, cap {cap:?})");
    assert!(new_429 <= 80, "steady state should rarely hit the limit: {new_429} 429s");
    assert!(p.limiter().limit() >= 16, "concurrency limit collapsed to {}", p.limiter().limit());
}

#[tokio::test]
async fn overloaded_provider_converges_to_capacity_then_recovers() {
    // Provider: 8 concurrent at 10ms, latency rising with overload, hard limit 24 → 503.
    // Engine deliberately misconfigured to start at 128 concurrent (5x too many).
    let up = upstream(Behaviour { capacity: 8, hard_limit: 24, base_latency_ms: 10, ..Default::default() }).await;
    let mut cfg = settings(&up.url());
    cfg.adaptive =
        AdaptiveConfig { min: 2, max: 256, initial: 128, cooldown_min: Duration::from_millis(5), ..Default::default() };
    let p = Arc::new(Provider::new("t", cfg).unwrap());
    // Phase 1: discovery. Early calls may fail (retry budget refuses to amplify overload).
    let _ = fire(&p, 1500, 128, true).await;
    let converged = p.limiter().limit();
    assert!((8..=32).contains(&converged), "limit converges near provider capacity (24): {converged}");
    // Phase 2: same overload, controller converged: nearly everything succeeds.
    let before = up.upstream.stats();
    let (ok, failed, _) = fire(&p, 1000, 128, true).await;
    let after = up.upstream.stats();
    assert!(ok as f64 / 1000.0 >= 0.99, "converged success rate: ok={ok} failed={failed}");
    let new_503 = after.unavailable - before.unavailable;
    assert!(new_503 < 50, "converged engine rarely overloads the provider: {new_503} 503s");
    // Phase 3: provider capacity grows; the controller climbs (additive increase while saturated).
    up.upstream.set(Behaviour { capacity: 1000, hard_limit: 100_000, base_latency_ms: 10, ..Default::default() });
    let (ok3, _, _) = fire(&p, 4000, 128, true).await;
    assert_eq!(ok3, 4000);
    assert!(p.limiter().limit() > converged + 4, "recovered: {} > {}", p.limiter().limit(), converged);
}

#[tokio::test]
async fn outage_opens_circuit_fails_fast_then_recovers() {
    let up = upstream(Behaviour { outage: true, ..Default::default() }).await;
    let mut cfg = settings(&up.url());
    cfg.retry.max_retries = 0;
    cfg.breaker = BreakerConfig {
        min_requests: 10,
        open_for: Duration::from_millis(200),
        soft_min_span: Duration::from_millis(300),
        probes: 2,
        ..Default::default()
    };
    let p = Arc::new(Provider::new("t", cfg).unwrap());
    // A sustained outage (every request 503 for longer than soft_min_span).
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(400) && p.health().circuit != "open" {
        let _ = p.call(CallRequest::post("/v1/echo", json!({}))).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(p.health().circuit, "open");
    let sent_before = up.upstream.stats().requests;
    let t = Instant::now();
    let r = p.call(CallRequest::post("/v1/echo", json!({}))).await;
    assert!(matches!(r, Err(CallError::CircuitOpen)), "{r:?}");
    assert!(t.elapsed() < Duration::from_millis(20), "fails fast without a network round trip");
    assert_eq!(up.upstream.stats().requests, sent_before, "nothing sent while open");
    up.upstream.set(Behaviour::default()); // provider recovers
    tokio::time::sleep(Duration::from_millis(250)).await;
    for _ in 0..3 {
        p.call(CallRequest::post("/v1/echo", json!({}))).await.unwrap();
    }
    assert_eq!(p.health().circuit, "closed");
}

#[tokio::test]
async fn non_idempotent_posts_are_not_retried() {
    let up = upstream(Behaviour { error_rate: 1.0, ..Default::default() }).await;
    let p = Arc::new(Provider::new("t", settings(&up.url())).unwrap());
    let r = p.call(CallRequest::post("/v1/echo", json!({}))).await;
    assert!(matches!(r, Err(CallError::Status { .. })));
    assert_eq!(up.upstream.stats().requests, 1, "a POST without an idempotency key is sent once");
    let r = p.call(CallRequest::post("/v1/echo", json!({})).idempotency_key("abc")).await;
    assert!(r.is_err());
    assert!(up.upstream.stats().requests >= 3, "keyed POSTs are retried");
}

#[tokio::test]
async fn deadlines_bound_total_time() {
    let up = upstream(Behaviour { slow_rate: 1.0, slow_ms: 3000, ..Default::default() }).await;
    let p = Arc::new(Provider::new("t", settings(&up.url())).unwrap());
    let t = Instant::now();
    let r = p
        .call(CallRequest::post("/v1/echo", json!({})).idempotency_key("x").deadline(Duration::from_millis(300)))
        .await;
    assert!(matches!(r, Err(CallError::Timeout | CallError::DeadlineExceeded)), "{r:?}");
    assert!(t.elapsed() < Duration::from_millis(800), "took {:?}", t.elapsed());
}

#[tokio::test]
async fn paths_cannot_escape_the_configured_base_url() {
    let up = upstream(Behaviour::default()).await;
    let p = Provider::new("t", settings(&up.url())).unwrap();
    for bad in ["http://169.254.169.254/latest/meta-data", "//evil.example/x", "/../admin", "relative"] {
        let r = p.call(CallRequest::post(bad, json!({}))).await;
        assert!(matches!(r, Err(CallError::Invalid(_))), "{bad}: {r:?}");
    }
    assert_eq!(up.upstream.stats().requests, 0);
}
