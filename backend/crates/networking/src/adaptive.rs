//! Adaptive concurrency (AIMD with a latency signal) and a priority permit queue.
//!
//! Algorithm (per provider), tuned to be conservative:
//! - **Overload signals**: 429, 503, 504, timeouts, or *sustained* latency above
//!   `latency_tolerance × baseline` (both the sample and a fast EWMA must exceed it).
//!   Response: `limit = max(min, limit × decrease_factor)`, at most once per *drain epoch*:
//!   after a decrease, the requests that were already in flight (admitted under the old limit)
//!   must complete before the next decrease. One burst of concurrent failures caused by one
//!   overload event therefore shrinks the limit once, independent of latency (which may itself
//!   be inflated when traffic starts in overload).
//! - **Healthy success** while the limit is actually being used (inflight ≥ 80% of limit):
//!   `limit += 1/limit` (≈ +1 per limit's worth of successes: classic additive increase).
//!   Unused headroom is not grown, so an idle provider does not "remember" a huge limit.
//! - **Plain errors** (500, transport) do not move the limit; the circuit breaker handles them.
//! - **Baseline latency**: tracks good-condition latency (moves down quickly, up slowly).
//!
//! Permits are granted in priority order (then FIFO); the wait queue is bounded so callers get
//! fast `QueueFull` backpressure instead of unbounded memory growth.

use std::{
    cmp::Ordering,
    collections::BinaryHeap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::sync::oneshot;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdaptiveConfig {
    pub min: usize,
    pub max: usize,
    pub initial: usize,
    pub decrease_factor: f64,
    pub latency_tolerance: f64,
    pub max_queue: usize,
    pub cooldown_min: Duration,
}

impl Default for AdaptiveConfig {
    fn default() -> Self {
        Self {
            min: 1,
            max: 512,
            initial: 32,
            decrease_factor: 0.75,
            latency_tolerance: 2.0,
            max_queue: 10_000,
            cooldown_min: Duration::from_millis(100),
        }
    }
}

/// How a request went, as far as capacity is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    /// The provider signalled (or showed) overload: 429, 503, 504, timeout.
    Overload,
    /// Failure unrelated to load (500, malformed response, transport error).
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AcquireError {
    #[error("concurrency queue full")]
    QueueFull,
    #[error("deadline exceeded waiting for capacity")]
    Timeout,
}

struct Waiter {
    priority: u8,
    seq: u64,
    tx: oneshot::Sender<()>,
}

impl PartialEq for Waiter {
    fn eq(&self, o: &Self) -> bool {
        self.priority == o.priority && self.seq == o.seq
    }
}
impl Eq for Waiter {}
impl PartialOrd for Waiter {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Waiter {
    // Max-heap: higher priority first, then lower sequence (FIFO).
    fn cmp(&self, o: &Self) -> Ordering {
        self.priority.cmp(&o.priority).then_with(|| o.seq.cmp(&self.seq))
    }
}

struct State {
    limit: f64,
    inflight: usize,
    waiters: BinaryHeap<Waiter>,
    seq: u64,
    last_decrease: Option<Instant>,
    /// Completions still owed by requests admitted before the last decrease.
    epoch_remaining: usize,
    baseline_ms: Option<f64>,
    fast_ms: Option<f64>,
}

pub struct AdaptiveLimiter {
    cfg: AdaptiveConfig,
    state: Mutex<State>,
}

/// A held concurrency slot. Record the outcome with [`Permit::finish`]; dropping it without
/// finishing releases the slot without feeding the controller (e.g. cancellation).
pub struct Permit {
    limiter: Arc<AdaptiveLimiter>,
    started: Instant,
    done: bool,
}

impl Permit {
    pub fn finish(mut self, outcome: Outcome) {
        let latency = self.started.elapsed();
        self.done = true;
        self.limiter.release(Some((outcome, latency)));
    }
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if !self.done {
            self.limiter.release(None);
        }
    }
}

struct WaitGuard {
    rx: Option<oneshot::Receiver<()>>,
    limiter: Arc<AdaptiveLimiter>,
}

impl WaitGuard {
    /// Stop listening; true if a grant had already been delivered (caller now owns the slot).
    fn take_raced_grant(&mut self) -> bool {
        match self.rx.take() {
            Some(mut rx) => {
                rx.close();
                rx.try_recv().is_ok()
            }
            None => false,
        }
    }
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        // Cancelled while waiting: a grant that already arrived must be handed back.
        if self.take_raced_grant() {
            self.limiter.release(None);
        }
    }
}

impl AdaptiveLimiter {
    pub fn new(cfg: AdaptiveConfig) -> Arc<Self> {
        let initial = cfg.initial.clamp(cfg.min.max(1), cfg.max.max(1));
        Arc::new(Self {
            cfg,
            state: Mutex::new(State {
                limit: initial as f64,
                inflight: 0,
                waiters: BinaryHeap::new(),
                seq: 0,
                last_decrease: None,
                epoch_remaining: 0,
                baseline_ms: None,
                fast_ms: None,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn limit(&self) -> usize {
        self.lock().limit.floor() as usize
    }
    pub fn inflight(&self) -> usize {
        self.lock().inflight
    }
    pub fn queued(&self) -> usize {
        self.lock().waiters.len()
    }
    pub fn baseline_ms(&self) -> Option<f64> {
        self.lock().baseline_ms
    }

    /// Acquire a slot, waiting up to `deadline` in priority order.
    pub async fn acquire(self: &Arc<Self>, priority: u8, deadline: Instant) -> Result<Permit, AcquireError> {
        let rx = {
            let mut s = self.lock();
            if s.inflight < (s.limit.floor() as usize).max(1) && s.waiters.is_empty() {
                s.inflight += 1;
                return Ok(Permit { limiter: self.clone(), started: Instant::now(), done: false });
            }
            if s.waiters.len() >= self.cfg.max_queue {
                return Err(AcquireError::QueueFull);
            }
            let (tx, rx) = oneshot::channel();
            s.seq += 1;
            let seq = s.seq;
            s.waiters.push(Waiter { priority, seq, tx });
            rx
        };
        // The guard closes the channel on timeout *or cancellation* and returns any grant that
        // raced in (sent after our last poll but before we stopped listening) so no slot leaks.
        let mut guard = WaitGuard { rx: Some(rx), limiter: self.clone() };
        let granted = tokio::select! {
            biased;
            r = guard.rx.as_mut().expect("present") => r.is_ok(),
            () = tokio::time::sleep_until(deadline.into()) => false,
        };
        if granted {
            guard.rx = None;
            return Ok(Permit { limiter: self.clone(), started: Instant::now(), done: false });
        }
        if guard.take_raced_grant() {
            return Ok(Permit { limiter: self.clone(), started: Instant::now(), done: false });
        }
        Err(AcquireError::Timeout)
    }

    fn release(&self, sample: Option<(Outcome, Duration)>) {
        let mut s = self.lock();
        s.inflight = s.inflight.saturating_sub(1);
        if let Some((outcome, latency)) = sample {
            self.on_sample(&mut s, outcome, latency);
        }
        self.grant(&mut s);
        metrics::gauge!("app_provider_queue_depth_internal").set(s.waiters.len() as f64);
    }

    fn grant(&self, s: &mut State) {
        while s.inflight < (s.limit.floor() as usize).max(1) {
            let Some(w) = s.waiters.pop() else { break };
            s.inflight += 1;
            if w.tx.send(()).is_err() {
                s.inflight -= 1; // waiter gave up (timeout/cancel): skip it
            }
        }
    }

    fn on_sample(&self, s: &mut State, outcome: Outcome, latency: Duration) {
        s.epoch_remaining = s.epoch_remaining.saturating_sub(1);
        let ms = latency.as_secs_f64() * 1000.0;
        match outcome {
            Outcome::Success => {
                // Slow EWMA (α = 0.1): one slow response is noise; several in a row are a trend.
                s.fast_ms = Some(s.fast_ms.map_or(ms, |f| 0.9 * f + 0.1 * ms));
                s.baseline_ms = Some(match s.baseline_ms {
                    None => ms,
                    Some(b) if ms < b => 0.9 * b + 0.1 * ms,
                    Some(b) => 0.995 * b + 0.005 * ms,
                });
                let base = s.baseline_ms.unwrap_or(ms).max(0.05);
                let sustained_slow = ms > base * self.cfg.latency_tolerance
                    && s.fast_ms.unwrap_or(0.0) > base * self.cfg.latency_tolerance;
                if sustained_slow {
                    self.decrease(s);
                } else if s.inflight as f64 + 1.0 >= 0.8 * s.limit {
                    s.limit = (s.limit + 1.0 / s.limit.max(1.0)).min(self.cfg.max as f64);
                }
            }
            Outcome::Overload => self.decrease(s),
            Outcome::Error => {}
        }
    }

    fn decrease(&self, s: &mut State) {
        let now = Instant::now();
        let spaced = s.last_decrease.is_none_or(|t| now.duration_since(t) >= self.cfg.cooldown_min);
        if s.epoch_remaining == 0 && spaced {
            s.limit = (s.limit * self.cfg.decrease_factor).max(self.cfg.min.max(1) as f64);
            s.last_decrease = Some(now);
            // Everything currently in flight was admitted under the old limit.
            s.epoch_remaining = s.inflight;
        }
    }

    /// Test/diagnostic hook: feed a synthetic sample without holding a permit.
    #[doc(hidden)]
    pub fn __sample(&self, outcome: Outcome, latency: Duration) {
        let mut s = self.lock();
        self.on_sample(&mut s, outcome, latency);
    }
    #[doc(hidden)]
    pub fn __set_inflight(&self, n: usize) {
        self.lock().inflight = n;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn cfg() -> AdaptiveConfig {
        AdaptiveConfig { min: 2, max: 1000, initial: 100, cooldown_min: Duration::ZERO, ..Default::default() }
    }

    #[test]
    fn overload_decreases_multiplicatively_and_respects_min() {
        let l = AdaptiveLimiter::new(cfg());
        l.__sample(Outcome::Overload, Duration::from_millis(10));
        assert_eq!(l.limit(), 75);
        for _ in 0..100 {
            l.__sample(Outcome::Overload, Duration::from_millis(10));
        }
        assert_eq!(l.limit(), 2, "never below min");
    }

    #[test]
    fn one_overload_event_is_one_decrease() {
        let l = AdaptiveLimiter::new(cfg());
        l.__set_inflight(50); // 50 requests in flight when the provider starts returning 429
        for _ in 0..50 {
            l.__sample(Outcome::Overload, Duration::from_millis(10));
        }
        assert_eq!(l.limit(), 75, "those 50 failures are one event: one decrease");
        l.__sample(Outcome::Overload, Duration::from_millis(10)); // a request admitted after it
        assert_eq!(l.limit(), 56, "the next epoch may decrease again");
    }

    #[test]
    fn healthy_saturated_traffic_grows_additively_but_idle_does_not() {
        let l = AdaptiveLimiter::new(cfg());
        l.__set_inflight(0); // idle
        for _ in 0..1000 {
            l.__sample(Outcome::Success, Duration::from_millis(10));
        }
        assert_eq!(l.limit(), 100, "unused capacity is not grown");
        l.__set_inflight(99); // saturated
        for _ in 0..1000 {
            l.__sample(Outcome::Success, Duration::from_millis(10));
        }
        let grown = l.limit();
        assert!((105..=115).contains(&grown), "≈ +1 per `limit` successes, got {grown}");
    }

    #[test]
    fn sustained_latency_growth_is_an_overload_signal_but_a_blip_is_not() {
        let l = AdaptiveLimiter::new(cfg());
        l.__set_inflight(99);
        for _ in 0..200 {
            l.__sample(Outcome::Success, Duration::from_millis(10));
        }
        let before = l.limit();
        l.__sample(Outcome::Success, Duration::from_millis(100)); // single blip
        assert!(l.limit() >= before, "a single slow response is tolerated");
        for _ in 0..20 {
            l.__sample(Outcome::Success, Duration::from_millis(100));
        }
        assert!(l.limit() < before, "sustained 10x latency shrinks the limit");
    }

    #[test]
    fn errors_do_not_move_the_limit() {
        let l = AdaptiveLimiter::new(cfg());
        for _ in 0..100 {
            l.__sample(Outcome::Error, Duration::from_millis(10));
        }
        assert_eq!(l.limit(), 100);
    }

    #[tokio::test]
    async fn permits_are_granted_by_priority_then_fifo() {
        let l = AdaptiveLimiter::new(AdaptiveConfig { initial: 1, min: 1, ..cfg() });
        let far = Instant::now() + Duration::from_secs(5);
        let held = l.acquire(0, far).await.unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut handles = Vec::new();
        for (name, prio) in [("low-1", 1u8), ("high", 9), ("low-2", 1), ("mid", 5)] {
            let (l, order) = (l.clone(), order.clone());
            handles.push(tokio::spawn(async move {
                let p = l.acquire(prio, far).await.unwrap();
                order.lock().unwrap().push(name);
                drop(p);
            }));
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(l.queued(), 4);
        drop(held);
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(*order.lock().unwrap(), vec!["high", "mid", "low-1", "low-2"]);
    }

    #[tokio::test]
    async fn queue_is_bounded_and_deadlines_are_respected() {
        let l = AdaptiveLimiter::new(AdaptiveConfig { initial: 1, min: 1, max_queue: 1, ..cfg() });
        let soon = Instant::now() + Duration::from_millis(50);
        let _held = l.acquire(0, soon).await.unwrap();
        let waiter = {
            let l = l.clone();
            tokio::spawn(async move { l.acquire(0, soon).await.map(|_| ()) })
        };
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(l.acquire(0, soon).await.err(), Some(AcquireError::QueueFull));
        assert_eq!(waiter.await.unwrap(), Err(AcquireError::Timeout));
        // the timed-out waiter's slot is not leaked
        drop(_held);
        assert_eq!(l.inflight(), 0);
        assert!(l.acquire(0, Instant::now() + Duration::from_millis(50)).await.is_ok());
    }

    proptest! {
        /// Under any sequence of outcomes the limit stays within [min, max].
        #[test]
        fn limit_always_within_bounds(ops in proptest::collection::vec((0u8..3, 1u64..500), 1..500)) {
            let l = AdaptiveLimiter::new(AdaptiveConfig { min: 3, max: 64, initial: 20, cooldown_min: Duration::ZERO, ..Default::default() });
            l.__set_inflight(19);
            for (o, ms) in ops {
                let outcome = match o { 0 => Outcome::Success, 1 => Outcome::Overload, _ => Outcome::Error };
                l.__sample(outcome, Duration::from_millis(ms));
                let lim = l.limit();
                prop_assert!((3..=64).contains(&lim), "limit {} out of bounds", lim);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod cancel_tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_waiters_never_leak_slots() {
        let l = AdaptiveLimiter::new(AdaptiveConfig { initial: 1, min: 1, max: 1, ..Default::default() });
        let far = Instant::now() + Duration::from_secs(5);
        for _ in 0..200 {
            let held = l.acquire(0, far).await.unwrap();
            let w = {
                let l = l.clone();
                tokio::spawn(async move { l.acquire(0, far).await.map(|_| ()) })
            };
            tokio::task::yield_now().await;
            drop(held); // grant goes to the waiter...
            w.abort(); // ...which may be cancelled at the same moment
            let _ = w.await;
        }
        assert_eq!(l.inflight(), 0, "no slot leaked across 200 grant/cancel races");
        assert!(l.acquire(0, Instant::now() + Duration::from_millis(100)).await.is_ok());
    }
}
