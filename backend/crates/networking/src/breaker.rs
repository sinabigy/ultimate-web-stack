//! Circuit breaker: stop sending to a failing provider, probe for recovery.
//!
//! Closed: outcomes counted over a rolling window; opens when ≥ `min_requests` samples and
//! either *hard* failures (500/502, transport errors, timeouts) reach `failure_ratio`, or hard +
//! *soft* failures (503/504 backpressure) reach `unavailable_ratio` (a real outage). Partial
//! overload (many 503s, some successes) is left to the adaptive concurrency controller. Open: calls fail fast for `open_for` (doubling on each
//! consecutive re-open, capped). Half-open: up to `probes` trial calls; all succeed → closed,
//! any failure → open again. Rate limiting (429) is *not* a failure (the AIMD controller and
//! Retry-After handle it); 5xx, timeouts and transport errors are.

use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy)]
pub struct BreakerConfig {
    pub window: Duration,
    pub min_requests: usize,
    pub failure_ratio: f64,
    pub unavailable_ratio: f64,
    /// Backpressure must be sustained this long before it opens the circuit (fast 503s arrive
    /// before slow successes, so a short window over-counts them).
    pub soft_min_span: Duration,
    pub open_for: Duration,
    pub max_open_for: Duration,
    pub probes: usize,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(10),
            min_requests: 20,
            failure_ratio: 0.5,
            unavailable_ratio: 0.95,
            soft_min_span: Duration::from_secs(1),
            open_for: Duration::from_secs(5),
            max_open_for: Duration::from_secs(60),
            probes: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

impl CircuitState {
    pub fn as_str(self) -> &'static str {
        match self {
            CircuitState::Closed => "closed",
            CircuitState::Open => "open",
            CircuitState::HalfOpen => "half_open",
        }
    }
}

/// What a completed call means for provider health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Success,
    /// Backpressure (503/504): overloaded, not broken.
    Soft,
    /// Broken: 500/502, transport errors, timeouts.
    Hard,
}

enum S {
    Closed(VecDeque<(Instant, Health)>),
    Open { until: Instant, opens: u32 },
    HalfOpen { started: usize, ok: usize, opens: u32 },
}

pub struct CircuitBreaker {
    cfg: BreakerConfig,
    s: Mutex<S>,
}

impl CircuitBreaker {
    pub fn new(cfg: BreakerConfig) -> Self {
        Self { cfg, s: Mutex::new(S::Closed(VecDeque::new())) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, S> {
        self.s.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn state(&self) -> CircuitState {
        match &*self.lock() {
            S::Closed(_) => CircuitState::Closed,
            S::Open { until, .. } if *until > Instant::now() => CircuitState::Open,
            S::Open { .. } | S::HalfOpen { .. } => CircuitState::HalfOpen,
        }
    }

    /// May a call proceed? Half-open admits a limited number of probes.
    pub fn allow(&self) -> bool {
        let mut s = self.lock();
        match &mut *s {
            S::Closed(_) => true,
            S::Open { until, opens } => {
                if *until > Instant::now() {
                    false
                } else {
                    let opens = *opens;
                    *s = S::HalfOpen { started: 1, ok: 0, opens };
                    true
                }
            }
            S::HalfOpen { started, .. } => {
                if *started < self.cfg.probes {
                    *started += 1;
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn record(&self, outcome: Health) {
        let success = outcome == Health::Success;
        let now = Instant::now();
        let mut s = self.lock();
        let next = match &mut *s {
            S::Closed(win) => {
                win.push_back((now, outcome));
                while win.front().is_some_and(|(t, _)| now.duration_since(*t) > self.cfg.window) {
                    win.pop_front();
                }
                let total = win.len() as f64;
                let hard = win.iter().filter(|(_, h)| *h == Health::Hard).count() as f64;
                let soft = win.iter().filter(|(_, h)| *h == Health::Soft).count() as f64;
                let span = win.front().map_or(Duration::ZERO, |(t, _)| now.duration_since(*t));
                let broken = hard / total >= self.cfg.failure_ratio;
                let unavailable = (hard + soft) / total >= self.cfg.unavailable_ratio && span >= self.cfg.soft_min_span;
                (win.len() >= self.cfg.min_requests && (broken || unavailable)).then(|| self.open(0))
            }
            S::Open { .. } => None,
            S::HalfOpen { ok, opens, .. } => {
                if !success {
                    Some(self.open(*opens + 1))
                } else {
                    *ok += 1;
                    (*ok >= self.cfg.probes).then(|| S::Closed(VecDeque::new()))
                }
            }
        };
        if let Some(n) = next {
            if matches!(n, S::Open { .. }) {
                metrics::counter!("app_provider_circuit_opened_total").increment(1);
            }
            *s = n;
        }
    }

    fn open(&self, opens: u32) -> S {
        let factor = 2u32.saturating_pow(opens.min(16));
        let d = self.cfg.open_for.saturating_mul(factor).min(self.cfg.max_open_for);
        S::Open { until: Instant::now() + d, opens }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn quick() -> BreakerConfig {
        BreakerConfig { min_requests: 4, open_for: Duration::from_millis(30), probes: 2, ..Default::default() }
    }

    #[test]
    fn opens_after_failure_ratio_then_half_opens_and_closes() {
        let b = CircuitBreaker::new(quick());
        for ok in [true, false, false, false] {
            assert!(b.allow());
            b.record(if ok { Health::Success } else { Health::Hard });
        }
        assert_eq!(b.state(), CircuitState::Open);
        assert!(!b.allow(), "fails fast while open");
        std::thread::sleep(Duration::from_millis(40));
        assert!(b.allow(), "first probe");
        assert!(b.allow(), "second probe");
        assert!(!b.allow(), "probe budget exhausted");
        b.record(Health::Success);
        b.record(Health::Success);
        assert_eq!(b.state(), CircuitState::Closed);
    }

    #[test]
    fn failed_probe_reopens_for_longer() {
        let b = CircuitBreaker::new(quick());
        for _ in 0..4 {
            b.allow();
            b.record(Health::Hard);
        }
        std::thread::sleep(Duration::from_millis(40));
        assert!(b.allow());
        b.record(Health::Hard);
        assert_eq!(b.state(), CircuitState::Open);
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(b.state(), CircuitState::Open, "second open lasts 2x");
        std::thread::sleep(Duration::from_millis(30));
        assert!(b.allow());
    }

    #[test]
    fn partial_overload_does_not_trip_but_total_unavailability_does() {
        let b = CircuitBreaker::new(quick());
        for i in 0..40 {
            b.record(if i % 5 == 0 { Health::Success } else { Health::Soft }); // 80% 503s
        }
        assert_eq!(b.state(), CircuitState::Closed, "overload is the concurrency controller's job");
        let b = CircuitBreaker::new(BreakerConfig { soft_min_span: Duration::from_millis(30), ..quick() });
        for _ in 0..20 {
            b.record(Health::Soft); // 100% 503 ...
        }
        assert_eq!(b.state(), CircuitState::Closed, "... but not yet sustained");
        std::thread::sleep(Duration::from_millis(35));
        b.record(Health::Soft);
        assert_eq!(b.state(), CircuitState::Open, "sustained total unavailability is an outage");
    }

    #[test]
    fn needs_minimum_volume() {
        let b = CircuitBreaker::new(quick());
        for _ in 0..3 {
            b.record(Health::Hard);
        }
        assert_eq!(b.state(), CircuitState::Closed, "3 failures < min_requests");
    }
}
