//! In-process cache (moka): bounded, per-entry TTL, lock-free reads. Per instance only: with
//! several instances each has its own copy (fine for short-TTL aggregates; use Redis for shared
//! state such as rate limits or locks that must hold across instances).

use std::time::{Duration, Instant};

use bytes::Bytes;
use moka::{Expiry, future::Cache as Moka};

use crate::{Cache, CacheError, LockToken};

#[derive(Clone)]
struct Entry {
    value: Bytes,
    ttl: Duration,
}

struct PerEntryTtl;

impl Expiry<String, Entry> for PerEntryTtl {
    fn expire_after_create(&self, _: &String, e: &Entry, _: Instant) -> Option<Duration> {
        Some(e.ttl)
    }
    fn expire_after_update(&self, _: &String, e: &Entry, _: Instant, _: Option<Duration>) -> Option<Duration> {
        Some(e.ttl)
    }
}

pub struct MemoryCache {
    inner: Moka<String, Entry>,
}

impl MemoryCache {
    pub fn new(max_entries: u64) -> Self {
        Self { inner: Moka::builder().max_capacity(max_entries).expire_after(PerEntryTtl).build() }
    }
}

#[async_trait::async_trait]
impl Cache for MemoryCache {
    fn backend(&self) -> &'static str {
        "memory"
    }
    async fn get(&self, key: &str) -> Result<Option<Bytes>, CacheError> {
        Ok(self.inner.get(key).await.map(|e| e.value))
    }
    async fn set(&self, key: &str, value: Bytes, ttl: Duration) -> Result<(), CacheError> {
        self.inner.insert(key.to_string(), Entry { value, ttl }).await;
        Ok(())
    }
    async fn delete(&self, key: &str) -> Result<(), CacheError> {
        self.inner.invalidate(key).await;
        Ok(())
    }
    async fn try_lock(&self, key: &str, ttl: Duration) -> Result<Option<LockToken>, CacheError> {
        let token = LockToken::random();
        let entry =
            self.inner.entry(key.to_string()).or_insert(Entry { value: Bytes::from(token.0.clone()), ttl }).await;
        Ok(entry.is_fresh().then_some(token))
    }
    async fn unlock(&self, key: &str, token: &LockToken) -> Result<(), CacheError> {
        if self.inner.get(key).await.is_some_and(|e| e.value == token.0.as_bytes()) {
            self.inner.invalidate(key).await;
        }
        Ok(())
    }
    async fn ping(&self) -> Result<(), CacheError> {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use crate::CacheLayer;

    #[tokio::test]
    async fn ttl_expiry_and_lock_ownership() {
        let c = MemoryCache::new(100);
        c.set("k", Bytes::from_static(b"v"), Duration::from_millis(50)).await.unwrap();
        assert_eq!(c.get("k").await.unwrap().as_deref(), Some(&b"v"[..]));
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(c.get("k").await.unwrap(), None);
        let t = c.try_lock("l", Duration::from_secs(5)).await.unwrap().unwrap();
        assert!(c.try_lock("l", Duration::from_secs(5)).await.unwrap().is_none(), "exclusive");
        c.unlock("l", &LockToken("not-mine".into())).await.unwrap();
        assert!(c.try_lock("l", Duration::from_secs(5)).await.unwrap().is_none(), "only the owner unlocks");
        c.unlock("l", &t).await.unwrap();
        assert!(c.try_lock("l", Duration::from_secs(5)).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn single_flight_prevents_stampede() {
        let layer = CacheLayer::new(Arc::new(MemoryCache::new(100)), "t");
        let calls = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..50 {
            let (layer, calls) = (layer.clone(), calls.clone());
            handles.push(tokio::spawn(async move {
                layer
                    .get_or_load("hot", Duration::from_secs(10), || async {
                        calls.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        Ok::<_, ()>(42u32)
                    })
                    .await
                    .unwrap()
            }));
        }
        for h in handles {
            assert_eq!(h.await.unwrap(), 42);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "50 concurrent misses → one load");
    }

    #[tokio::test]
    async fn loader_errors_propagate_and_are_not_cached() {
        let layer = CacheLayer::new(Arc::new(MemoryCache::new(100)), "t");
        let r: Result<u32, &str> = layer.get_or_load("k", Duration::from_secs(10), || async { Err("db down") }).await;
        assert_eq!(r, Err("db down"));
        let r: Result<u32, &str> = layer.get_or_load("k", Duration::from_secs(10), || async { Ok(7) }).await;
        assert_eq!(r, Ok(7));
    }

    #[test]
    fn key_convention() {
        let layer = CacheLayer::new(Arc::new(MemoryCache::new(1)), "app");
        assert_eq!(layer.key("org-stats", 2, &["abc", "14"]), "app:org-stats:v2:abc:14");
    }
}
