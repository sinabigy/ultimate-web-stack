//! Request-rate and token-budget limiting with reservation semantics (GCRA with delay).
//!
//! `reserve(cost, deadline)` computes the earliest time this request may proceed, reserves it,
//! and returns how long to wait; if that is past the deadline nothing is reserved and the call
//! fails immediately (so callers do not queue work they cannot finish). A provider-wide
//! `pause_until` (from `Retry-After`) applies to everyone.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("rate budget would exceed the request deadline")]
pub struct WouldExceedDeadline;

pub struct RateLimiter {
    params: Mutex<(Option<f64>, f64)>, // (seconds per unit of cost = 1/rate or None, burst tolerance τ seconds)
    state: Mutex<(Instant, Option<Instant>)>, // (theoretical arrival time, paused until)
}

impl RateLimiter {
    /// `per_second` ≤ 0 means unlimited. `burst` is in cost units.
    pub fn new(per_second: f64, burst: f64) -> Self {
        Self { params: Mutex::new(Self::params(per_second, burst)), state: Mutex::new((Instant::now(), None)) }
    }

    fn params(per_second: f64, burst: f64) -> (Option<f64>, f64) {
        let emission = (per_second > 0.0).then(|| 1.0 / per_second);
        (emission, emission.map_or(0.0, |t| t * burst.max(1.0)))
    }

    /// Change the rate at runtime (adaptive rate control). `per_second` ≤ 0 = unlimited.
    pub fn set_rate(&self, per_second: f64, burst: f64) {
        *self.params.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Self::params(per_second, burst);
    }

    /// Current rate (requests/s) or None when unlimited.
    pub fn rate(&self) -> Option<f64> {
        self.params.lock().ok().and_then(|p| p.0).map(|t| 1.0 / t)
    }

    pub fn unlimited() -> Self {
        Self::new(0.0, 0.0)
    }

    /// Reserve `cost` units; returns the delay to wait before proceeding.
    pub fn reserve(&self, cost: f64, deadline: Instant) -> Result<Duration, WouldExceedDeadline> {
        let now = Instant::now();
        let (emission, tau) = *self.params.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut g = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let paused = g.1.filter(|p| *p > now).unwrap_or(now);
        let Some(t) = emission else {
            return if paused > deadline { Err(WouldExceedDeadline) } else { Ok(paused - now) };
        };
        let tat = g.0.max(now);
        let increment = Duration::from_secs_f64(t * cost.max(0.0));
        // GCRA with delay: admissible once (tat + cost·T) − t ≤ τ, i.e. t ≥ tat + cost·T − τ.
        let earliest = (tat + increment).checked_sub(Duration::from_secs_f64(tau)).unwrap_or(now).max(now).max(paused);
        if earliest > deadline {
            return Err(WouldExceedDeadline);
        }
        g.0 = tat.max(earliest) + increment;
        Ok(earliest - now)
    }

    /// Pause everyone until `until` (e.g. after a 429 with `Retry-After`).
    pub fn pause_until(&self, until: Instant) {
        let mut g = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.1 = Some(g.1.map_or(until, |p| p.max(until)));
    }

    pub fn paused_until(&self) -> Option<Instant> {
        self.state.lock().ok().and_then(|g| g.1).filter(|p| *p > Instant::now())
    }

    pub async fn acquire(&self, cost: f64, deadline: Instant) -> Result<(), WouldExceedDeadline> {
        let wait = self.reserve(cost, deadline)?;
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn burst_then_paced() {
        let r = RateLimiter::new(10.0, 3.0);
        let far = Instant::now() + Duration::from_secs(60);
        let waits: Vec<Duration> = (0..6).map(|_| r.reserve(1.0, far).unwrap()).collect();
        assert!(waits[..3].iter().all(|w| w.is_zero()), "burst of 3 immediate: {waits:?}");
        assert!(waits[3] > Duration::from_millis(50) && waits[5] > waits[4], "then paced at 10/s: {waits:?}");
    }

    #[test]
    fn deadline_is_respected_without_reserving() {
        let r = RateLimiter::new(1.0, 1.0);
        let soon = Instant::now() + Duration::from_millis(100);
        assert!(r.reserve(1.0, soon).is_ok());
        assert_eq!(r.reserve(1.0, soon), Err(WouldExceedDeadline));
        assert_eq!(r.reserve(1.0, soon), Err(WouldExceedDeadline), "failed attempts reserve nothing");
    }

    #[test]
    fn token_budget_costs_scale() {
        // 6000 tokens/min = 100 tokens/s, burst 1000 tokens
        let r = RateLimiter::new(100.0, 1000.0);
        let far = Instant::now() + Duration::from_secs(120);
        assert!(r.reserve(1000.0, far).unwrap().is_zero());
        let w = r.reserve(500.0, far).unwrap();
        assert!(w >= Duration::from_millis(4900) && w <= Duration::from_millis(5100), "{w:?}");
    }

    #[test]
    fn pause_applies_even_when_unlimited() {
        let r = RateLimiter::unlimited();
        r.pause_until(Instant::now() + Duration::from_secs(2));
        let w = r.reserve(1.0, Instant::now() + Duration::from_secs(10)).unwrap();
        assert!(w > Duration::from_millis(1900));
        assert!(r.reserve(1.0, Instant::now() + Duration::from_millis(500)).is_err());
    }
}
