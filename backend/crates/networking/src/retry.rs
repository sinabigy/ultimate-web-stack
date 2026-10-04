//! Retry policy: exponential backoff with full jitter, `Retry-After` awareness, and a retry
//! budget so retries cannot multiply load during an outage (retry storms).

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base: Duration,
    pub max_backoff: Duration,
    /// Retries allowed as a fraction of recent first attempts (plus `min_per_window`).
    pub budget_ratio: f64,
    pub min_per_window: u32,
    pub window: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base: Duration::from_millis(100),
            max_backoff: Duration::from_secs(10),
            budget_ratio: 0.2,
            min_per_window: 10,
            window: Duration::from_secs(10),
        }
    }
}

impl RetryPolicy {
    /// Full-jitter backoff for retry number `attempt` (1-based), at least `retry_after`.
    pub fn backoff(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        let cap =
            self.base.saturating_mul(2u32.saturating_pow(attempt.saturating_sub(1).min(20))).min(self.max_backoff);
        let mut b = [0u8; 8];
        rand::fill(&mut b);
        let frac = u64::from_le_bytes(b) as f64 / u64::MAX as f64;
        let jittered = cap.mul_f64(frac);
        retry_after.map_or(jittered, |ra| ra.max(jittered))
    }
}

/// Tracks first attempts and retries over a rolling window.
pub struct RetryBudget {
    policy: RetryPolicy,
    state: Mutex<(Instant, u32, u32)>, // window start, attempts, retries
}

impl RetryBudget {
    pub fn new(policy: RetryPolicy) -> Self {
        Self { policy, state: Mutex::new((Instant::now(), 0, 0)) }
    }

    fn roll(&self, g: &mut (Instant, u32, u32)) {
        if g.0.elapsed() > self.policy.window {
            *g = (Instant::now(), 0, 0);
        }
    }

    pub fn record_attempt(&self) {
        let mut g = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.roll(&mut g);
        g.1 += 1;
    }

    /// Spend one retry from the budget if available.
    pub fn try_spend(&self) -> bool {
        let mut g = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.roll(&mut g);
        let allowed = self.policy.min_per_window as f64 + self.policy.budget_ratio * g.1 as f64;
        if (g.2 as f64) < allowed {
            g.2 += 1;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_jittered_capped_and_honours_retry_after() {
        let p = RetryPolicy::default();
        for attempt in 1..10 {
            let b = p.backoff(attempt, None);
            assert!(b <= p.max_backoff);
            assert!(b <= p.base * 2u32.pow(attempt - 1));
        }
        assert!(p.backoff(1, Some(Duration::from_secs(3))) >= Duration::from_secs(3));
    }

    #[test]
    fn budget_limits_retry_storms() {
        let b = RetryBudget::new(RetryPolicy { min_per_window: 2, budget_ratio: 0.1, ..Default::default() });
        for _ in 0..100 {
            b.record_attempt();
        }
        let granted = (0..100).filter(|_| b.try_spend()).count();
        assert_eq!(granted, 12, "2 + 10% of 100 attempts");
    }
}
