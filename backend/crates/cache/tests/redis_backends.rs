#![allow(clippy::unwrap_used)]
//! Redis and Dragonfly integration. Runs when TEST_REDIS_URLS is set (comma separated), e.g.
//! TEST_REDIS_URLS=redis://127.0.0.1:56379,redis://127.0.0.1:56380 (./dev up starts both).

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use app_cache::{
    Cache, CacheLayer,
    redis::{RedisCache, RedisRateLimiter},
};
use app_rate_limit::{Decision, RateLimiter};
use bytes::Bytes;

fn urls() -> Vec<String> {
    match std::env::var("TEST_REDIS_URLS") {
        Ok(v) if !v.is_empty() => v.split(',').map(str::to_string).collect(),
        _ => {
            eprintln!("SKIPPED: set TEST_REDIS_URLS to run Redis/Dragonfly integration tests");
            Vec::new()
        }
    }
}

fn unique(p: &str) -> String {
    format!("test:{p}:{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
}

#[tokio::test]
async fn basic_ops_ttl_and_locks() {
    for url in urls() {
        let c = RedisCache::connect(&url).await.unwrap();
        c.ping().await.unwrap();
        let k = unique("k");
        c.set(&k, Bytes::from_static(b"v"), Duration::from_millis(200)).await.unwrap();
        assert_eq!(c.get(&k).await.unwrap().as_deref(), Some(&b"v"[..]), "{url}");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(c.get(&k).await.unwrap(), None, "{url}: TTL");
        let l = unique("lock");
        let t = c.try_lock(&l, Duration::from_secs(5)).await.unwrap().unwrap();
        assert!(c.try_lock(&l, Duration::from_secs(5)).await.unwrap().is_none(), "{url}: exclusive");
        c.unlock(&l, &app_cache::LockToken("intruder".into())).await.unwrap();
        assert!(c.try_lock(&l, Duration::from_secs(5)).await.unwrap().is_none(), "{url}: owner-only unlock");
        c.unlock(&l, &t).await.unwrap();
        assert!(c.try_lock(&l, Duration::from_secs(5)).await.unwrap().is_some(), "{url}");
    }
}

#[tokio::test]
async fn distributed_rate_limit_is_shared_by_instances() {
    for url in urls() {
        let c = RedisCache::connect(&url).await.unwrap();
        let prefix = unique("rl");
        // Two limiter objects = two app instances sharing one limit of 5 burst / 10 per second.
        let a = RedisRateLimiter::new(&c, &prefix, 10.0, 5);
        let b = RedisRateLimiter::new(&c, &prefix, 10.0, 5);
        let mut allowed = 0;
        for i in 0..10 {
            let d = if i % 2 == 0 { a.check("client-1").await.unwrap() } else { b.check("client-1").await.unwrap() };
            if d.is_allowed() {
                allowed += 1;
            }
        }
        assert_eq!(allowed, 5, "{url}: burst shared across instances");
        let d = a.check("client-1").await.unwrap();
        assert!(
            matches!(d, Decision::Deny { retry_after } if retry_after <= Duration::from_millis(100)),
            "{url}: {d:?}"
        );
        assert!(a.check("client-2").await.unwrap().is_allowed(), "{url}: per key");
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(b.check("client-1").await.unwrap().is_allowed(), "{url}: refills");
    }
}

#[tokio::test]
async fn stampede_protection_across_processes() {
    for url in urls() {
        // Separate layers = separate processes (separate single-flight maps), shared Redis.
        let layers: Vec<CacheLayer> =
            futures_join(4, |_| async { CacheLayer::new(Arc::new(RedisCache::connect(&url).await.unwrap()), "t") })
                .await;
        let loads = Arc::new(AtomicUsize::new(0));
        let key = unique("hot");
        let mut handles = Vec::new();
        for i in 0..40 {
            let (layer, loads, key) = (layers[i % 4].clone(), loads.clone(), key.clone());
            handles.push(tokio::spawn(async move {
                layer
                    .get_or_load(&key, Duration::from_secs(30), || async {
                        loads.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        Ok::<_, ()>(serde_json::json!({"v": 1}))
                    })
                    .await
                    .unwrap()
            }));
        }
        for h in handles {
            assert_eq!(h.await.unwrap()["v"], 1);
        }
        assert_eq!(loads.load(Ordering::SeqCst), 1, "{url}: 40 concurrent misses across 4 processes → 1 load");
    }
}

async fn futures_join<T, F, Fut>(n: usize, f: F) -> Vec<T>
where
    F: Fn(usize) -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let mut v = Vec::new();
    for i in 0..n {
        v.push(f(i).await);
    }
    v
}

/// TCP proxy we can cut, to simulate the cache server disappearing mid-flight.
async fn proxy(target: &str) -> (String, tokio::sync::watch::Sender<bool>) {
    let addr = target.trim_start_matches("redis://").to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        loop {
            let Ok((mut inbound, _)) = listener.accept().await else { return };
            let addr = addr.clone();
            let mut stop = rx.clone();
            tokio::spawn(async move {
                if *stop.borrow() {
                    return; // "server down": accept then drop
                }
                let Ok(mut outbound) = tokio::net::TcpStream::connect(&addr).await else { return };
                tokio::select! {
                    _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound) => {}
                    _ = stop.changed() => {}
                }
            });
        }
    });
    (format!("redis://{local}"), tx)
}

#[tokio::test]
async fn cache_outage_degrades_to_source_without_failing_requests() {
    for url in urls() {
        let (via, cut) = proxy(&url).await;
        let layer = CacheLayer::new(Arc::new(RedisCache::connect(&via).await.unwrap()), "t");
        let key = unique("o");
        let v: Result<u32, ()> = layer.get_or_load(&key, Duration::from_secs(30), || async { Ok(1) }).await;
        assert_eq!(v, Ok(1));
        cut.send(true).unwrap(); // cache server "dies"
        let t = std::time::Instant::now();
        let v: Result<u32, ()> = layer.get_or_load(&key, Duration::from_secs(30), || async { Ok(2) }).await;
        assert_eq!(v, Ok(2), "{url}: served from the source of truth");
        assert!(t.elapsed() < Duration::from_secs(3), "{url}: degraded quickly ({:?})", t.elapsed());
    }
}
