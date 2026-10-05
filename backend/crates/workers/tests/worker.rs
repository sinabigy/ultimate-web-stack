#![allow(clippy::unwrap_used)]
//! Job runtime + run execution against PostgreSQL and the simulated provider.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use app_authz::{Actor, OrgAccess, PermissionSet, UserActor};
use app_db::{jobs, orgs, runs, users};
use app_domain::{NewRun, RealtimeEvent, RunStatus, SystemRole};
use app_messaging::EventBus;
use app_networking::{
    AdaptiveConfig, Provider, ProviderRegistry, ProviderSettings, breaker::BreakerConfig, retry::RetryPolicy,
};
use app_workers::{JobServices, PgWorker, WorkerConfig, events::PgEventBus, handlers::ExecuteRun};
use fake_upstream::{Behaviour, start};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

async fn setup(pool: &PgPool) -> (Uuid, Uuid) {
    let (u, _) = users::upsert_from_login(
        pool,
        &users::IdentityClaims {
            issuer: "https://idp",
            subject: "s",
            email: "w@example.com",
            email_verified: true,
            name: Some("W"),
            picture: None,
        },
        false,
    )
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let org = orgs::ensure_personal_org(&mut tx, u.id, "W").await.unwrap();
    tx.commit().await.unwrap();
    (u.id, org.id)
}

fn access(org: Uuid, user: Uuid) -> OrgAccess {
    let a = Actor::User(UserActor {
        id: user,
        system_role: SystemRole::None,
        active: true,
        mfa: false,
        auth_age: Duration::ZERO,
    });
    OrgAccess::__for_tests(org, a, PermissionSet::all())
}

async fn new_run(pool: &PgPool, org: Uuid, user: Uuid, n: i32) -> Uuid {
    let run = runs::create(
        pool,
        &access(org, user),
        Some(user),
        &NewRun { label: "t".into(), provider: "simulated".into(), requested: n },
    )
    .await
    .unwrap();
    jobs::enqueue(
        pool,
        &jobs::NewJob {
            queue: "runs",
            kind: "execute_run",
            payload: serde_json::json!({"run_id": run.id, "organization_id": org}),
            priority: 0,
            max_attempts: 5,
            run_at: None,
            idempotency_key: Some(&format!("run:{}", run.id)),
            organization_id: Some(org),
            trace_context: None,
        },
    )
    .await
    .unwrap();
    run.id
}

fn provider(url: String) -> ProviderRegistry {
    ProviderRegistry::with(vec![Arc::new(
        Provider::new(
            "simulated",
            ProviderSettings {
                base_url: url,
                api_key: None,
                adaptive: AdaptiveConfig { initial: 16, max: 64, ..Default::default() },
                requests_per_second: 0.0,
                burst: 0,
                tokens_per_minute: 0,
                request_timeout: Duration::from_secs(5),
                connect_timeout: Duration::from_secs(2),
                retry: RetryPolicy::default(),
                breaker: BreakerConfig::default(),
                http2_prior_knowledge: false,
                pool_idle_timeout: Duration::from_secs(90),
                dns_cache: false,
            },
        )
        .unwrap(),
    )])
}

async fn services(pool: &PgPool, url: String) -> JobServices {
    JobServices {
        db: pool.clone(),
        events: PgEventBus::start(pool.clone()).await.unwrap(),
        providers: provider(url),
        analytics: Arc::new(app_analytics::NoopSink),
    }
}

fn worker(svc: JobServices, poll: Duration) -> PgWorker {
    let mut cfg = WorkerConfig::new("runs", 4);
    cfg.poll_interval = poll;
    cfg.shutdown_grace = Duration::from_secs(5);
    PgWorker::new(cfg, svc, vec![Arc::new(ExecuteRun { parallelism: 8 })])
}

async fn wait_status(pool: &PgPool, org: Uuid, user: Uuid, run: Uuid, want: RunStatus, secs: u64) -> app_domain::Run {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let r = runs::get(pool, &access(org, user), run).await.unwrap();
        if r.status == want || std::time::Instant::now() > deadline {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn run_executes_with_progress_events_and_notification(pool: PgPool) {
    let (user, org) = setup(&pool).await;
    let up = start(SocketAddr::from(([127, 0, 0, 1], 0)), Behaviour { base_latency_ms: 2, ..Default::default() })
        .await
        .unwrap();
    let svc = services(&pool, up.url()).await;
    let mut events = svc.events.subscribe();
    let run_id = new_run(&pool, org, user, 120).await;
    let stop = CancellationToken::new();
    let w = tokio::spawn(worker(svc, Duration::from_secs(1)).run(stop.clone()));
    let run = wait_status(&pool, org, user, run_id, RunStatus::Completed, 20).await;
    assert_eq!((run.status, run.succeeded, run.failed), (RunStatus::Completed, 120, 0));
    assert_eq!(up.upstream.stats().distinct_idempotency_keys, 120);
    let (mut progress, mut finished, mut notified) = (0, false, false);
    while let Ok(Ok(ev)) = tokio::time::timeout(Duration::from_millis(500), events.recv()).await {
        match ev {
            RealtimeEvent::RunProgress { run_id: r, .. } if r == run_id => progress += 1,
            RealtimeEvent::RunFinished { run_id: r, status, .. } if r == run_id => {
                finished = status == RunStatus::Completed
            }
            RealtimeEvent::Notification { user_id, .. } if user_id == user => notified = true,
            _ => {}
        }
    }
    assert!(progress >= 5, "progress events: {progress}");
    assert!(finished && notified);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id = $1")
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    let status: String = sqlx::query_scalar("SELECT status FROM jobs LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(status, "succeeded");
    stop.cancel();
    w.await.unwrap();
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn interrupted_run_resumes_on_another_worker_without_redoing_work(pool: PgPool) {
    let (user, org) = setup(&pool).await;
    let up = start(SocketAddr::from(([127, 0, 0, 1], 0)), Behaviour { base_latency_ms: 15, ..Default::default() })
        .await
        .unwrap();
    let run_id = new_run(&pool, org, user, 200).await;
    // Worker 1 starts, then the process is asked to shut down mid-run.
    let stop1 = CancellationToken::new();
    let w1 = tokio::spawn(worker(services(&pool, up.url()).await, Duration::from_millis(200)).run(stop1.clone()));
    tokio::time::sleep(Duration::from_millis(150)).await;
    stop1.cancel();
    w1.await.unwrap();
    let mid = runs::get(&pool, &access(org, user), run_id).await.unwrap();
    assert!(mid.succeeded > 0 && mid.succeeded < 200, "partial progress checkpointed: {}", mid.succeeded);
    let (status, attempts): (String, i32) =
        sqlx::query_as("SELECT status, attempts FROM jobs").fetch_one(&pool).await.unwrap();
    assert_eq!((status.as_str(), attempts), ("queued", 1), "interrupted job is requeued, not lost");
    sqlx::query("UPDATE jobs SET run_at = now()").execute(&pool).await.unwrap(); // skip the backoff for the test
    // Worker 2 (a different process in production) resumes.
    let stop2 = CancellationToken::new();
    let w2 = tokio::spawn(worker(services(&pool, up.url()).await, Duration::from_millis(200)).run(stop2.clone()));
    let done = wait_status(&pool, org, user, run_id, RunStatus::Completed, 30).await;
    assert_eq!((done.status, done.succeeded), (RunStatus::Completed, 200));
    // Calls in flight at interruption may be re-sent, but with the same idempotency keys.
    assert_eq!(up.upstream.stats().distinct_idempotency_keys, 200);
    assert!(
        up.upstream.stats().requests < 200 + 16,
        "at most the in-flight calls were repeated: {}",
        up.upstream.stats().requests
    );
    stop2.cancel();
    w2.await.unwrap();
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn poison_and_unknown_jobs_are_dead_lettered(pool: PgPool) {
    for (kind, payload) in
        [("execute_run", serde_json::json!({"garbage": true})), ("no_such_kind", serde_json::json!({}))]
    {
        jobs::enqueue(
            &pool,
            &jobs::NewJob {
                queue: "runs",
                kind,
                payload,
                priority: 0,
                max_attempts: 5,
                run_at: None,
                idempotency_key: None,
                organization_id: None,
                trace_context: None,
            },
        )
        .await
        .unwrap();
    }
    let up = start(SocketAddr::from(([127, 0, 0, 1], 0)), Behaviour::default()).await.unwrap();
    let stop = CancellationToken::new();
    let w = tokio::spawn(worker(services(&pool, up.url()).await, Duration::from_millis(100)).run(stop.clone()));
    tokio::time::sleep(Duration::from_millis(800)).await;
    stop.cancel();
    w.await.unwrap();
    let rows: Vec<(String, i32, Option<String>)> =
        sqlx::query_as("SELECT status, attempts, last_error FROM jobs ORDER BY kind").fetch_all(&pool).await.unwrap();
    assert!(rows.iter().all(|(s, a, _)| s == "dead" && *a == 1), "dead after one attempt: {rows:?}");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn notify_wakes_idle_worker_without_waiting_for_poll(pool: PgPool) {
    let (user, org) = setup(&pool).await;
    let up = start(SocketAddr::from(([127, 0, 0, 1], 0)), Behaviour::default()).await.unwrap();
    let stop = CancellationToken::new();
    // Poll interval 30s: only LISTEN/NOTIFY can make this fast.
    let w = tokio::spawn(worker(services(&pool, up.url()).await, Duration::from_secs(30)).run(stop.clone()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let t = std::time::Instant::now();
    let run_id = new_run(&pool, org, user, 1).await;
    let r = wait_status(&pool, org, user, run_id, RunStatus::Completed, 5).await;
    assert_eq!(r.status, RunStatus::Completed);
    assert!(t.elapsed() < Duration::from_secs(2), "picked up in {:?}", t.elapsed());
    stop.cancel();
    w.await.unwrap();
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn pg_event_bus_delivers_across_processes(pool: PgPool) {
    let a = PgEventBus::start(pool.clone()).await.unwrap(); // e.g. worker process
    let b = PgEventBus::start(pool.clone()).await.unwrap(); // e.g. API instance
    let mut rx = b.subscribe();
    a.publish(RealtimeEvent::Heartbeat { at_unix_ms: 42 }).await;
    let ev = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
    assert_eq!(ev, RealtimeEvent::Heartbeat { at_unix_ms: 42 });
}

#[cfg(feature = "nats")]
#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn nats_unavailable_at_startup_falls_back_to_postgres_events(pool: PgPool) {
    let cfg = app_config::MessagingConfig {
        enabled: true,
        nats_url: "nats://127.0.0.1:9".into(), // nothing listens here
        ..Default::default()
    };
    let bus = app_workers::events::start_event_bus(&cfg, pool, "test").await.expect("starts without NATS");
    let mut rx = bus.subscribe();
    tokio::time::sleep(Duration::from_millis(200)).await; // LISTEN registered
    bus.publish(RealtimeEvent::Heartbeat { at_unix_ms: 7 }).await;
    let got = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("delivered").unwrap();
    assert_eq!(got, RealtimeEvent::Heartbeat { at_unix_ms: 7 }, "events still flow (over PostgreSQL)");
}
