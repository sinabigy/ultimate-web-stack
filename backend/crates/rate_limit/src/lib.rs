//! Rate limiting with the Generic Cell Rate Algorithm (GCRA).
//!
//! GCRA is a token bucket expressed as a single timestamp per key — the *theoretical
//! arrival time* (TAT) — so it needs O(1) state, has no refill timer and is trivially
//! portable to a shared store (Redis implements the same arithmetic in a Lua script, see
//! `app-cache`). For a rate `r` per second and burst `b`:
//!
//! ```text
//! T   = 1/r                   (emission interval)
//! tau = T * b                 (burst tolerance)
//! allow at time t  iff  max(TAT, t) - t <= tau - T      (i.e. the bucket has a token)
//! then TAT := max(TAT, t) + T
//! ```
//!
//! The limiter is per *client key* (IP, user id, API key id); choose keys server-side,
//! never from unauthenticated client-supplied identifiers.

use std::{
    collections::HashMap,
    hash::{BuildHasher, RandomState},
    sync::Mutex,
    time::{Duration, Instant},
};

/// Result of a rate-limit check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow { remaining: u32 },
    Deny { retry_after: Duration },
}

impl Decision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Decision::Allow { .. })
    }
}

/// Quota: sustained `per_second` with bursts up to `burst` requests.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quota {
    pub per_second: f64,
    pub burst: u32,
}

impl Quota {
    fn emission(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.per_second)
    }
    fn tolerance(&self) -> Duration {
        self.emission().mul_f64(f64::from(self.burst))
    }
}

/// Pure GCRA step. `tat` is the stored theoretical arrival time (offset from an epoch).
/// Returns the decision and the new TAT to store (unchanged on deny).
pub fn gcra(quota: Quota, tat: Option<Duration>, now: Duration) -> (Decision, Duration) {
    let t = quota.emission();
    let tau = quota.tolerance();
    let tat = tat.unwrap_or(now).max(now);
    let new_tat = tat + t;
    // allowed iff new_tat - now <= tau
    let used = new_tat.saturating_sub(now);
    if used <= tau {
        let remaining_time = tau - used;
        let remaining = (remaining_time.as_secs_f64() / t.as_secs_f64()).floor() as u32;
        (Decision::Allow { remaining }, new_tat)
    } else {
        (Decision::Deny { retry_after: used - tau }, tat)
    }
}

#[async_trait::async_trait]
pub trait RateLimiter: Send + Sync {
    /// Check and consume one unit for `key`. Implementations must **fail closed only for
    /// abuse-relevant keys** — on backend errors they return `Err` and the caller decides
    /// (the HTTP layer allows the request and records a metric; see docs/security).
    async fn check(&self, key: &str) -> Result<Decision, RateLimitError>;
}

#[derive(Debug)]
pub struct RateLimitError(pub String);

impl std::fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rate limiter backend error: {}", self.0)
    }
}

impl std::error::Error for RateLimitError {}

const SHARDS: usize = 64;

/// In-process GCRA limiter: sharded maps to keep lock contention low under high concurrency.
/// Per instance: with N instances behind a load balancer the effective limit is up to N×quota.
/// Use the Redis backend when a global limit matters.
pub struct MemoryRateLimiter {
    quota: Quota,
    epoch: Instant,
    hasher: RandomState,
    shards: Vec<Mutex<HashMap<String, Duration>>>,
}

impl MemoryRateLimiter {
    pub fn new(quota: Quota) -> Self {
        assert!(quota.per_second > 0.0 && quota.burst > 0, "quota must be positive");
        Self {
            quota,
            epoch: Instant::now(),
            hasher: RandomState::new(),
            shards: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect(),
        }
    }

    fn shard(&self, key: &str) -> &Mutex<HashMap<String, Duration>> {
        &self.shards[(self.hasher.hash_one(key) as usize) % SHARDS]
    }

    pub fn check_at(&self, key: &str, now: Duration) -> Decision {
        let mut map = self.shard(key).lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (d, tat) = gcra(self.quota, map.get(key).copied(), now);
        if d.is_allowed() {
            map.insert(key.to_owned(), tat);
        }
        d
    }

    /// Drop keys whose TAT is in the past (they are equivalent to absent keys).
    /// Call periodically; returns the number of keys removed.
    pub fn purge_expired(&self) -> usize {
        let now = self.epoch.elapsed();
        self.shards
            .iter()
            .map(|s| {
                let mut m = s.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let before = m.len();
                m.retain(|_, tat| *tat > now);
                before - m.len()
            })
            .sum()
    }

    pub fn len(&self) -> usize {
        self.shards.iter().map(|s| s.lock().map(|m| m.len()).unwrap_or(0)).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Spawn a background task that purges expired keys every `every`.
    pub fn spawn_janitor(self: &std::sync::Arc<Self>, every: Duration) -> tokio::task::JoinHandle<()> {
        let weak = std::sync::Arc::downgrade(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            loop {
                tick.tick().await;
                match weak.upgrade() {
                    Some(l) => {
                        l.purge_expired();
                    }
                    None => return,
                }
            }
        })
    }
}

#[async_trait::async_trait]
impl RateLimiter for MemoryRateLimiter {
    async fn check(&self, key: &str) -> Result<Decision, RateLimitError> {
        Ok(self.check_at(key, self.epoch.elapsed()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const Q: Quota = Quota { per_second: 10.0, burst: 5 };

    #[test]
    fn allows_burst_then_denies_then_recovers() {
        let l = MemoryRateLimiter::new(Q);
        let t0 = Duration::from_secs(100);
        for _ in 0..5 {
            assert!(l.check_at("a", t0).is_allowed());
        }
        match l.check_at("a", t0) {
            Decision::Deny { retry_after } => assert!(retry_after <= Duration::from_millis(100)),
            d => panic!("expected deny, got {d:?}"),
        }
        // other keys are independent
        assert!(l.check_at("b", t0).is_allowed());
        // one emission interval later exactly one more is allowed
        assert!(l.check_at("a", t0 + Duration::from_millis(100)).is_allowed());
        assert!(!l.check_at("a", t0 + Duration::from_millis(100)).is_allowed());
    }

    #[test]
    fn purge_removes_idle_keys() {
        let l = MemoryRateLimiter::new(Quota { per_second: 1000.0, burst: 1 });
        l.check_at("x", Duration::ZERO);
        assert_eq!(l.len(), 1);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(l.purge_expired(), 1);
        assert!(l.is_empty());
    }

    proptest! {
        /// Over any window, admitted requests never exceed burst + rate * window.
        #[test]
        fn never_exceeds_quota(gaps in proptest::collection::vec(0u64..50, 1..400),
                               rate in 1.0f64..200.0, burst in 1u32..50) {
            let q = Quota { per_second: rate, burst };
            let mut tat = None;
            let mut now = Duration::from_secs(1);
            let start = now;
            let mut allowed = 0u64;
            for g in gaps {
                now += Duration::from_millis(g);
                let (d, t) = gcra(q, tat, now);
                if d.is_allowed() { allowed += 1; tat = Some(t); }
            }
            let window = (now - start).as_secs_f64();
            let bound = f64::from(burst) + rate * window + 1.0;
            prop_assert!((allowed as f64) <= bound, "allowed {} > bound {}", allowed, bound);
        }
    }
}
