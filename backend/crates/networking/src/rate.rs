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

/// How far ahead `acquire` commits a slot (see [`RateLimiter::reserve_within`]).
const RESERVE_HORIZON: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reservation {
    /// Slot reserved; proceed after this delay.
    Granted(Duration),
    /// Nothing reserved; ask again after this delay.
    NotYet(Duration),
}

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
    /// The outstanding backlog (theoretical arrival time ahead of now) is rescaled to the new
    /// rate, so the change takes effect immediately rather than after the old backlog drains.
    pub fn set_rate(&self, per_second: f64, burst: f64) {
        let new = Self::params(per_second, burst);
        let mut p = self.params.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut g = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        if g.0 > now {
            g.0 = match (p.0, new.0) {
                (Some(old_t), Some(new_t)) => now + (g.0 - now).mul_f64(new_t / old_t),
                _ => now,
            };
        }
        *p = new;
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
        match self.reserve_within(cost, deadline, Duration::MAX)? {
            Reservation::Granted(wait) | Reservation::NotYet(wait) => Ok(wait),
        }
    }

    /// Like [`reserve`](Self::reserve) but only commits a slot that starts within `horizon`;
    /// otherwise returns when to ask again. Keeping reservations short-range means a rate change
    /// (adaptive control) applies to waiting callers within `horizon`, instead of everyone
    /// keeping a slot computed at the old rate.
    pub fn reserve_within(
        &self,
        cost: f64,
        deadline: Instant,
        horizon: Duration,
    ) -> Result<Reservation, WouldExceedDeadline> {
        let now = Instant::now();
        let (emission, tau) = *self.params.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut g = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let paused = g.1.filter(|p| *p > now).unwrap_or(now);
        let Some(t) = emission else {
            return if paused > deadline { Err(WouldExceedDeadline) } else { Ok(Reservation::Granted(paused - now)) };
        };
        let tat = g.0.max(now);
        let increment = Duration::from_secs_f64(t * cost.max(0.0));
        // GCRA with delay: admissible once (tat + cost·T) − t ≤ τ, i.e. t ≥ tat + cost·T − τ.
        let earliest = (tat + increment).checked_sub(Duration::from_secs_f64(tau)).unwrap_or(now).max(now).max(paused);
        if earliest > deadline {
            return Err(WouldExceedDeadline);
        }
        if earliest - now > horizon {
            return Ok(Reservation::NotYet(earliest - now - horizon));
        }
        g.0 = tat.max(earliest) + increment;
        Ok(Reservation::Granted(earliest - now))
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
        loop {
            match self.reserve_within(cost, deadline, RESERVE_HORIZON)? {
                Reservation::Granted(wait) => {
                    if !wait.is_zero() {
                        tokio::time::sleep(wait).await;
                    }
                    return Ok(());
                }
                Reservation::NotYet(wait) => tokio::time::sleep(wait.max(Duration::from_millis(1))).await,
            }
        }
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
    fn short_horizon_reservations_follow_rate_changes() {
        let r = RateLimiter::new(10.0, 1.0);
        let far = Instant::now() + Duration::from_secs(60);
        let h = Duration::from_millis(50);
        assert_eq!(r.reserve_within(1.0, far, h).unwrap(), Reservation::Granted(Duration::ZERO));
        // Next slot is ~100ms away at 10/s: not committed.
        assert!(matches!(r.reserve_within(1.0, far, h).unwrap(), Reservation::NotYet(_)));
        // Raise the rate: the next slot moves closer because nothing was committed at the old rate.
        r.set_rate(1000.0, 1.0);
        assert!(
            matches!(r.reserve_within(1.0, far, h).unwrap(), Reservation::Granted(w) if w < Duration::from_millis(101))
        );
    }

    #[test]
    fn pause_applies_even_when_unlimited() {
        let r = RateLimiter::unlimited();
        r.pause_until(Instant::now() + Duration::from_secs(2));
        let w = r.reserve(1.0, Instant::now() + Duration::from_secs(10)).unwrap();
        assert!(w > Duration::from_millis(1900));
        assert!(r.reserve(1.0, Instant::now() + Duration::from_millis(500)).is_err());
    }

    proptest::proptest! {
        /// GCRA conformance: for any rate, burst and sequence of reservations, any two granted
        /// slots i < j satisfy (j − i + 1) ≤ burst + rate × (t_j − t_i).
        #[test]
        fn granted_slots_conform_to_rate_and_burst(rate in 1.0f64..2000.0, burst in 1u32..50, n in 1usize..250) {
            let r = RateLimiter::new(rate, f64::from(burst));
            let far = Instant::now() + Duration::from_secs(3600);
            let mut slots = Vec::with_capacity(n);
            for _ in 0..n {
                let now = Instant::now();
                let wait = r.reserve(1.0, far).unwrap();
                slots.push(now + wait);
            }
            slots.sort();
            for i in 0..slots.len() {
                for j in (i + 1)..slots.len() {
                    let span = (slots[j] - slots[i]).as_secs_f64();
                    // j − i + 1 grants in a closed span: at most burst + rate × span (exact GCRA
                    // bound). The test reads the clock just before the limiter does, so spans may
                    // measure up to a few µs short: allow 50 µs (an off-by-one is a whole extra
                    // grant, i.e. ≥ 500 µs at the highest tested rate).
                    let allowed = f64::from(burst) + rate * (span + 50e-6) + 1e-9;
                    proptest::prop_assert!(((j - i + 1) as f64) <= allowed,
                        "{} grants in {:.6}s at {}/s burst {}", j - i, span, rate, burst);
                }
            }
        }

        /// Raising or lowering the rate never produces a slot earlier than "now" and keeps
        /// subsequent grants conformant to the new rate.
        #[test]
        fn rate_changes_keep_conformance(r1 in 5.0f64..500.0, r2 in 5.0f64..500.0, n in 2usize..120) {
            let r = RateLimiter::new(r1, 1.0);
            let far = Instant::now() + Duration::from_secs(3600);
            for _ in 0..n { r.reserve(1.0, far).unwrap(); }
            r.set_rate(r2, 1.0);
            let mut slots = Vec::new();
            for _ in 0..n {
                let now = Instant::now();
                let w = r.reserve(1.0, far).unwrap();
                slots.push(now + w);
            }
            for k in 1..slots.len() {
                let gap = (slots[k] - slots[k - 1]).as_secs_f64();
                proptest::prop_assert!(gap + 1e-6 >= 1.0 / r2 - 1e-3, "gap {gap} < 1/{r2}");
            }
        }
    }
}
