//! Caching.
//!
//! **The cache is never the source of truth.** Every value can be recomputed from PostgreSQL;
//! cache failures degrade to a miss (counted in `app_cache_errors_total`) and never fail a
//! request. Keys are namespaced `<namespace>:<domain>:<version>:<id…>`; bump `<version>` when a
//! value's shape changes.
//!
//! Backends: [`memory::MemoryCache`] (core profile, per-instance) and [`redis::RedisCache`]
//! (performance profile; Redis, Valkey or Dragonfly via RESP).
//!
//! [`CacheLayer::get_or_load`] adds stampede protection: per-process single-flight, optional
//! cross-process lock, and TTL jitter so many keys do not expire at the same instant.

pub mod memory;
pub mod redis;

use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

use bytes::Bytes;
use serde::{Serialize, de::DeserializeOwned};

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("cache backend unavailable: {0}")]
    Unavailable(String),
    #[error("cache value could not be (de)serialised: {0}")]
    Codec(String),
}

#[async_trait::async_trait]
pub trait Cache: Send + Sync {
    fn backend(&self) -> &'static str;
    async fn get(&self, key: &str) -> Result<Option<Bytes>, CacheError>;
    async fn set(&self, key: &str, value: Bytes, ttl: Duration) -> Result<(), CacheError>;
    async fn delete(&self, key: &str) -> Result<(), CacheError>;
    /// Acquire a short-lived exclusive lock. `Ok(None)` = held by someone else.
    async fn try_lock(&self, key: &str, ttl: Duration) -> Result<Option<LockToken>, CacheError>;
    async fn unlock(&self, key: &str, token: &LockToken) -> Result<(), CacheError>;
    async fn ping(&self) -> Result<(), CacheError>;
}

/// Opaque proof of lock ownership (random value): only the owner can release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockToken(pub String);

impl LockToken {
    pub fn random() -> Self {
        let mut b = [0u8; 16];
        rand::fill(&mut b);
        Self(b.iter().map(|x| format!("{x:02x}")).collect())
    }
}

/// Typed, namespaced, stampede-protected access on top of a [`Cache`].
#[derive(Clone)]
pub struct CacheLayer {
    cache: Arc<dyn Cache>,
    namespace: String,
    flights: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    cross_process_lock: bool,
}

impl CacheLayer {
    pub fn new(cache: Arc<dyn Cache>, namespace: &str) -> Self {
        let cross = cache.backend() != "memory";
        Self { cache, namespace: namespace.to_string(), flights: Arc::default(), cross_process_lock: cross }
    }

    pub fn backend(&self) -> &'static str {
        self.cache.backend()
    }

    pub fn raw(&self) -> &Arc<dyn Cache> {
        &self.cache
    }

    /// `<namespace>:<domain>:<version>:<parts…>`
    pub fn key(&self, domain: &str, version: u32, parts: &[&str]) -> String {
        let mut k = format!("{}:{domain}:v{version}", self.namespace);
        for p in parts {
            k.push(':');
            k.push_str(p);
        }
        k
    }

    fn jittered(ttl: Duration) -> Duration {
        let mut b = [0u8; 2];
        rand::fill(&mut b);
        let f = 0.9 + (u16::from_le_bytes(b) as f64 / u16::MAX as f64) * 0.2; // ±10%
        ttl.mul_f64(f)
    }

    async fn read<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        match self.cache.get(key).await {
            Ok(Some(b)) => match serde_json::from_slice(&b) {
                Ok(v) => {
                    metrics::counter!("app_cache_requests_total", "result" => "hit").increment(1);
                    Some(v)
                }
                Err(e) => {
                    tracing::warn!(error = %e, key, "undecodable cache entry; treating as miss");
                    None
                }
            },
            Ok(None) => {
                metrics::counter!("app_cache_requests_total", "result" => "miss").increment(1);
                None
            }
            Err(e) => {
                metrics::counter!("app_cache_errors_total", "backend" => self.cache.backend()).increment(1);
                tracing::warn!(error = %e, "cache read failed; falling back to source");
                None
            }
        }
    }

    pub async fn put<T: Serialize + Sync>(&self, key: &str, value: &T, ttl: Duration) {
        let Ok(bytes) = serde_json::to_vec(value) else { return };
        if let Err(e) = self.cache.set(key, Bytes::from(bytes), Self::jittered(ttl)).await {
            metrics::counter!("app_cache_errors_total", "backend" => self.cache.backend()).increment(1);
            tracing::warn!(error = %e, "cache write failed");
        }
    }

    pub async fn invalidate(&self, key: &str) {
        if let Err(e) = self.cache.delete(key).await {
            metrics::counter!("app_cache_errors_total", "backend" => self.cache.backend()).increment(1);
            tracing::warn!(error = %e, key, "cache invalidation failed; entry will expire by TTL");
        }
    }

    /// Return the cached value or compute it once (per key, per process; and across processes
    /// when the backend is shared), store it with jittered TTL and return it.
    /// Loader errors propagate; cache errors never do.
    pub async fn get_or_load<T, E, F, Fut>(&self, key: &str, ttl: Duration, load: F) -> Result<T, E>
    where
        T: Serialize + DeserializeOwned + Send + Sync,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        if let Some(v) = self.read(key).await {
            return Ok(v);
        }
        // Single-flight within this process.
        let flight = {
            let mut f = self.flights.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            f.entry(key.to_string()).or_default().clone()
        };
        let _guard = flight.lock().await;
        if let Some(v) = self.read(key).await {
            return Ok(v); // someone in this process filled it while we waited
        }
        // Across processes: briefly let one instance compute; others re-check.
        let lock_key = format!("{key}:lock");
        let mut token = None;
        if self.cross_process_lock {
            for _ in 0..20 {
                match self.cache.try_lock(&lock_key, Duration::from_secs(5)).await {
                    Ok(Some(t)) => {
                        token = Some(t);
                        break;
                    }
                    Ok(None) => {
                        tokio::time::sleep(Duration::from_millis(25)).await;
                        if let Some(v) = self.read(key).await {
                            return Ok(v);
                        }
                    }
                    Err(_) => break, // cache down: compute (availability over deduplication)
                }
            }
        }
        let result = load().await;
        if let Ok(v) = &result {
            self.put(key, v, ttl).await;
        }
        if let Some(t) = token {
            let _ = self.cache.unlock(&lock_key, &t).await;
        }
        {
            let mut f = self.flights.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if Arc::strong_count(&flight) <= 2 {
                f.remove(key);
            }
        }
        result
    }
}
