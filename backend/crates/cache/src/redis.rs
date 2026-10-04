//! Redis / Valkey / Dragonfly backend (RESP). Shared across instances: response cache, locks,
//! rate-limit state. Short timeouts so a slow cache cannot slow requests down; the connection
//! manager reconnects automatically after outages.

use std::time::Duration;

use app_rate_limit::{Decision, RateLimitError, RateLimiter};
use bytes::Bytes;
use redis::{AsyncCommands, Script, aio::ConnectionManager};

use crate::{Cache, CacheError, LockToken};

const UNLOCK: &str = r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) else return 0 end
"#;

/// GCRA in one atomic round trip using the server clock (consistent across all app instances).
/// ARGV: emission interval (µs), burst tolerance τ (µs). Returns {allowed, value} where value is
/// remaining tokens when allowed, or retry-after (µs) when denied.
const GCRA: &str = r#"
local t = redis.call('TIME')
local now = tonumber(t[1]) * 1000000 + tonumber(t[2])
local emission = tonumber(ARGV[1])
local tau = tonumber(ARGV[2])
local tat = tonumber(redis.call('GET', KEYS[1]) or now)
if tat < now then tat = now end
local new_tat = tat + emission
local used = new_tat - now
if used > tau then
  return {0, used - tau}
end
redis.call('SET', KEYS[1], new_tat, 'PX', math.ceil(used / 1000) + 1)
return {1, math.floor((tau - used) / emission)}
"#;

fn err(e: redis::RedisError) -> CacheError {
    CacheError::Unavailable(e.to_string())
}

#[derive(Clone)]
pub struct RedisCache {
    conn: ConnectionManager,
    unlock: Script,
}

impl RedisCache {
    pub async fn connect(url: &str) -> Result<Self, CacheError> {
        let client = redis::Client::open(url).map_err(err)?;
        let cfg = redis::aio::ConnectionManagerConfig::new()
            .set_connection_timeout(Some(Duration::from_millis(500)))
            .set_response_timeout(Some(Duration::from_millis(250)))
            .set_number_of_retries(2)
            .set_max_delay(Duration::from_secs(2));
        let conn = ConnectionManager::new_with_config(client, cfg).await.map_err(err)?;
        Ok(Self { conn, unlock: Script::new(UNLOCK) })
    }
}

#[async_trait::async_trait]
impl Cache for RedisCache {
    fn backend(&self) -> &'static str {
        "redis"
    }
    async fn get(&self, key: &str) -> Result<Option<Bytes>, CacheError> {
        let mut c = self.conn.clone();
        let v: Option<Vec<u8>> = c.get(key).await.map_err(err)?;
        Ok(v.map(Bytes::from))
    }
    async fn set(&self, key: &str, value: Bytes, ttl: Duration) -> Result<(), CacheError> {
        let mut c = self.conn.clone();
        let ms = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX).max(1);
        c.pset_ex::<_, _, ()>(key, value.as_ref(), ms).await.map_err(err)
    }
    async fn delete(&self, key: &str) -> Result<(), CacheError> {
        let mut c = self.conn.clone();
        c.del::<_, ()>(key).await.map_err(err)
    }
    async fn try_lock(&self, key: &str, ttl: Duration) -> Result<Option<LockToken>, CacheError> {
        let token = LockToken::random();
        let mut c = self.conn.clone();
        let ms = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX).max(1);
        let ok: Option<String> = redis::cmd("SET")
            .arg(key)
            .arg(&token.0)
            .arg("NX")
            .arg("PX")
            .arg(ms)
            .query_async(&mut c)
            .await
            .map_err(err)?;
        Ok(ok.map(|_| token))
    }
    async fn unlock(&self, key: &str, token: &LockToken) -> Result<(), CacheError> {
        let mut c = self.conn.clone();
        self.unlock.key(key).arg(&token.0).invoke_async::<i64>(&mut c).await.map(|_| ()).map_err(err)
    }
    async fn ping(&self) -> Result<(), CacheError> {
        let mut c = self.conn.clone();
        redis::cmd("PING").query_async::<String>(&mut c).await.map(|_| ()).map_err(err)
    }
}

/// Distributed GCRA rate limiter (same semantics as `app_rate_limit::MemoryRateLimiter`, shared
/// by all instances).
pub struct RedisRateLimiter {
    conn: ConnectionManager,
    script: Script,
    prefix: String,
    emission_us: u64,
    tau_us: u64,
}

impl RedisRateLimiter {
    pub fn new(cache: &RedisCache, prefix: &str, per_second: f64, burst: u32) -> Self {
        let emission_us = (1_000_000.0 / per_second.max(0.001)) as u64;
        Self {
            conn: cache.conn.clone(),
            script: Script::new(GCRA),
            prefix: prefix.to_string(),
            emission_us,
            tau_us: emission_us.saturating_mul(u64::from(burst.max(1))),
        }
    }
}

#[async_trait::async_trait]
impl RateLimiter for RedisRateLimiter {
    async fn check(&self, key: &str) -> Result<Decision, RateLimitError> {
        let mut c = self.conn.clone();
        let (allowed, value): (i64, i64) = self
            .script
            .key(format!("{}:rl:{key}", self.prefix))
            .arg(self.emission_us)
            .arg(self.tau_us)
            .invoke_async(&mut c)
            .await
            .map_err(|e| RateLimitError(e.to_string()))?;
        Ok(if allowed == 1 {
            Decision::Allow { remaining: u32::try_from(value.max(0)).unwrap_or(u32::MAX) }
        } else {
            Decision::Deny { retry_after: Duration::from_micros(u64::try_from(value.max(0)).unwrap_or(0)) }
        })
    }
}
