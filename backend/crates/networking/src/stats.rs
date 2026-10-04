//! Rolling per-provider statistics for health snapshots and the admin UI.

use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ok,
    RateLimited,
    ServerError,
    Timeout,
    Transport,
    ClientError,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthSnapshot {
    pub provider: String,
    pub concurrency_limit: usize,
    pub inflight: usize,
    pub queued: usize,
    pub circuit: &'static str,
    pub window_secs: u64,
    pub requests: usize,
    pub success_rate: f64,
    pub rate_429: f64,
    pub rate_5xx: f64,
    pub rate_timeout: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub throughput_rps: f64,
    pub paused_for_ms: u64,
    /// Current request-rate cap (learned or configured); None = unlimited.
    pub rate_cap_rps: Option<f64>,
}

pub struct Stats {
    window: Duration,
    cap: usize,
    samples: Mutex<VecDeque<(Instant, Kind, f64)>>,
}

impl Stats {
    pub fn new(window: Duration) -> Self {
        Self { window, cap: 20_000, samples: Mutex::new(VecDeque::new()) }
    }

    pub fn record(&self, kind: Kind, latency: Duration) {
        let now = Instant::now();
        let mut s = self.samples.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        s.push_back((now, kind, latency.as_secs_f64() * 1000.0));
        while s.len() > self.cap || s.front().is_some_and(|(t, _, _)| now.duration_since(*t) > self.window) {
            s.pop_front();
        }
    }

    /// (requests, success%, 429%, 5xx%, timeout%, p50, p95, p99, rps)
    pub fn summary(&self) -> (usize, f64, f64, f64, f64, f64, f64, f64, f64) {
        let now = Instant::now();
        let s = self.samples.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let live: Vec<_> = s.iter().filter(|(t, _, _)| now.duration_since(*t) <= self.window).collect();
        let n = live.len();
        if n == 0 {
            return (0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        }
        let frac = |k: Kind| live.iter().filter(|(_, kk, _)| *kk == k).count() as f64 / n as f64;
        let mut lat: Vec<f64> = live.iter().filter(|(_, k, _)| *k == Kind::Ok).map(|(_, _, l)| *l).collect();
        lat.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let pct = |p: f64| if lat.is_empty() { 0.0 } else { lat[((lat.len() - 1) as f64 * p).round() as usize] };
        let span = live.first().map_or(1.0, |(t, _, _)| now.duration_since(*t).as_secs_f64().max(1.0));
        (
            n,
            frac(Kind::Ok),
            frac(Kind::RateLimited),
            frac(Kind::ServerError),
            frac(Kind::Timeout),
            pct(0.5),
            pct(0.95),
            pct(0.99),
            n as f64 / span,
        )
    }

    /// Successful responses completed within `span` (observed accepted rate × span).
    pub fn successes_within(&self, span: Duration) -> usize {
        let now = Instant::now();
        let s = self.samples.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        s.iter().rev().take_while(|(t, _, _)| now.duration_since(*t) <= span).filter(|(_, k, _)| *k == Kind::Ok).count()
    }

    pub fn window(&self) -> Duration {
        self.window
    }
}
