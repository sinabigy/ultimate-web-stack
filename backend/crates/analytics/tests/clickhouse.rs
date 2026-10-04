#![allow(clippy::unwrap_used)]
//! ClickHouse integration. Runs when TEST_CLICKHOUSE_URL is set, e.g.
//! TEST_CLICKHOUSE_URL=http://127.0.0.1:58123 (user/password: TEST_CLICKHOUSE_USER /
//! TEST_CLICKHOUSE_PASSWORD, default app / app-dev-only). Each test uses its own database.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use app_analytics::{AnalyticsQuery, AnalyticsSink, ClickHouseSink, EventRow, SinkConfig, schema};
use app_authz::{Actor, OrgAccess, PermissionSet, UserActor};
use app_domain::SystemRole;
use uuid::Uuid;

struct TestDb {
    admin: clickhouse::Client,
    client: clickhouse::Client,
    name: String,
}

impl TestDb {
    async fn new() -> Option<Self> {
        let url = match std::env::var("TEST_CLICKHOUSE_URL") {
            Ok(u) if !u.is_empty() => u,
            _ => {
                eprintln!("TEST_CLICKHOUSE_URL not set: skipping ClickHouse integration test");
                return None;
            }
        };
        let user = std::env::var("TEST_CLICKHOUSE_USER").unwrap_or_else(|_| "app".into());
        let password = std::env::var("TEST_CLICKHOUSE_PASSWORD").unwrap_or_else(|_| "app-dev-only".into());
        static N: AtomicU64 = AtomicU64::new(0);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
        let name = format!("t_{}_{}", t % 100_000_000, N.fetch_add(1, Ordering::Relaxed));
        let admin = clickhouse::Client::default().with_url(&url).with_user(&user).with_password(&password);
        admin.query(&format!("CREATE DATABASE {name}")).execute().await.unwrap();
        let client = admin.clone().with_database(&name);
        Some(Self { admin, client, name })
    }

    async fn drop(self) {
        let _ = self.admin.query(&format!("DROP DATABASE IF EXISTS {}", self.name)).execute().await;
    }
}

fn access(org: Uuid) -> OrgAccess {
    let actor = Actor::User(UserActor {
        id: Uuid::nil(),
        system_role: SystemRole::None,
        active: true,
        mfa: false,
        auth_age: Duration::ZERO,
    });
    OrgAccess::__for_tests(org, actor, PermissionSet::all())
}

async fn count(c: &clickhouse::Client, sql: &str) -> u64 {
    c.query(sql).fetch_one::<u64>().await.unwrap()
}

#[tokio::test]
async fn schema_has_partitions_ttl_and_rollup_and_migrations_are_idempotent() {
    let Some(db) = TestDb::new().await else { return };
    assert_eq!(schema::migrate(&db.client).await.unwrap(), vec![1]);
    assert!(schema::migrate(&db.client).await.unwrap().is_empty(), "second run applies nothing");
    #[derive(clickhouse::Row, serde::Deserialize)]
    struct T {
        name: String,
        engine_full: String,
    }
    let tables: Vec<T> = db
        .client
        .query("SELECT name, engine_full FROM system.tables WHERE database = currentDatabase() ORDER BY name")
        .fetch_all()
        .await
        .unwrap();
    let get = |n: &str| tables.iter().find(|t| t.name == n).map(|t| t.engine_full.clone()).unwrap_or_default();
    let events = get("events");
    assert!(
        events.starts_with("MergeTree") && events.contains("PARTITION BY toYYYYMM(ts)") && events.contains("TTL"),
        "{events}"
    );
    assert!(get("events_daily").starts_with("SummingMergeTree"));
    assert!(tables.iter().any(|t| t.name == "events_daily_mv"));
    db.drop().await;
}

#[tokio::test]
async fn sink_batches_flushes_on_shutdown_and_rollup_matches() {
    let Some(db) = TestDb::new().await else { return };
    schema::migrate(&db.client).await.unwrap();
    let sink = ClickHouseSink::start(
        db.client.clone(),
        SinkConfig {
            batch_size: 5_000,
            flush_interval: Duration::from_millis(200),
            buffer_capacity: 100_000,
            insert_attempts: 3,
        },
    );
    let (org_a, org_b) = (Uuid::now_v7(), Uuid::now_v7());
    let t = Instant::now();
    for i in 0..30_000u32 {
        let org = if i % 3 == 0 { org_b } else { org_a };
        sink.record(EventRow::new("run_created", org).value(f64::from(i % 10)));
    }
    assert!(t.elapsed() < Duration::from_secs(1), "recording 30k events must not block: {:?}", t.elapsed());
    sink.shutdown().await;
    assert_eq!(count(&db.client, "SELECT count() FROM events").await, 30_000, "every buffered event flushed");
    // The rollup agrees with the raw table (sum: SummingMergeTree merges lazily).
    assert_eq!(count(&db.client, "SELECT sum(events) FROM events_daily").await, 30_000);
    let raw_value: f64 = db.client.query("SELECT sum(value) FROM events").fetch_one().await.unwrap();
    let rolled: f64 = db.client.query("SELECT sum(value) FROM events_daily").fetch_one().await.unwrap();
    assert!((raw_value - rolled).abs() < 1e-6);
    // Batched: far fewer parts than rows.
    let parts = count(
        &db.client,
        "SELECT count() FROM system.parts WHERE database = currentDatabase() AND table = 'events' AND active",
    )
    .await;
    assert!(parts <= 20, "{parts} parts for 30k rows");
    db.drop().await;
}

#[tokio::test]
async fn queries_are_tenant_scoped() {
    let Some(db) = TestDb::new().await else { return };
    schema::migrate(&db.client).await.unwrap();
    let sink = ClickHouseSink::start(
        db.client.clone(),
        SinkConfig {
            batch_size: 100,
            flush_interval: Duration::from_millis(50),
            buffer_capacity: 1_000,
            insert_attempts: 3,
        },
    );
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    for _ in 0..7 {
        sink.record(EventRow::new("run_created", a).value(10.0));
    }
    for _ in 0..3 {
        sink.record(EventRow::new("run_created", b).value(1.0));
    }
    sink.record(EventRow::new("other_event", a));
    sink.shutdown().await;
    let q = AnalyticsQuery::new(db.client.clone());
    let pa = q.daily(&access(a), "run_created", 30).await.unwrap();
    let pb = q.daily(&access(b), "run_created", 30).await.unwrap();
    assert_eq!(pa.len(), 1);
    assert_eq!((pa[0].events, pa[0].value), (7, 70.0), "only organisation A's events");
    assert_eq!((pb[0].events, pb[0].value), (3, 3.0));
    assert!(q.daily(&access(Uuid::now_v7()), "run_created", 30).await.unwrap().is_empty());
    db.drop().await;
}

#[tokio::test]
async fn a_full_buffer_drops_instead_of_blocking_and_an_outage_never_reaches_callers() {
    // ClickHouse unreachable (nothing listens on this port): inserts fail and are retried in
    // the background; callers are never blocked and nothing panics.
    let dead = clickhouse::Client::default().with_url("http://127.0.0.1:9");
    let sink = ClickHouseSink::start(
        dead,
        SinkConfig {
            batch_size: 10,
            flush_interval: Duration::from_millis(10),
            buffer_capacity: 50,
            insert_attempts: 2,
        },
    );
    let t = Instant::now();
    for _ in 0..100_000 {
        sink.record(EventRow::new("x", Uuid::nil()));
    }
    assert!(t.elapsed() < Duration::from_millis(500), "record() never waits: {:?}", t.elapsed());
    tokio::time::timeout(Duration::from_secs(30), sink.shutdown())
        .await
        .expect("shutdown completes despite the outage");
    sink.record(EventRow::new("after_shutdown", Uuid::nil())); // closed: dropped, no panic
}
