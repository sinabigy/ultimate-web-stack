//! `app-bench outbound|db [--smoke]` → JSON on stdout. Driven by benchmarks/run.py.
//!
//! Outbound scenarios run the engine *and* a naive client (no budgets, no adaptive control, no
//! breaker, immediate retries) against the same simulated provider, so the report can compare
//! **useful throughput** (completed work per second) and **waste** (requests the provider had to
//! reject), not just raw request rates.

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use app_networking::{
    AdaptiveConfig, CallRequest, Provider, ProviderSettings, breaker::BreakerConfig, retry::RetryPolicy,
};
use fake_upstream::{Behaviour, start};
use futures::{StreamExt, stream};
use serde_json::{Value, json};

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() { 0.0 } else { sorted[((sorted.len() - 1) as f64 * p).round() as usize] }
}

fn summarize(mut lat_ms: Vec<f64>, ok: u64, total: u64, wall: Duration) -> Value {
    lat_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    json!({
        "requests": total, "succeeded": ok, "success_rate": ok as f64 / total.max(1) as f64,
        "wall_s": wall.as_secs_f64(), "useful_rps": ok as f64 / wall.as_secs_f64(),
        "p50_ms": pct(&lat_ms, 0.5), "p95_ms": pct(&lat_ms, 0.95), "p99_ms": pct(&lat_ms, 0.99), "p999_ms": pct(&lat_ms, 0.999),
    })
}

fn settings(url: &str, adaptive: AdaptiveConfig) -> ProviderSettings {
    ProviderSettings {
        base_url: url.into(),
        api_key: None,
        adaptive,
        requests_per_second: 0.0,
        burst: 0,
        tokens_per_minute: 0,
        request_timeout: Duration::from_secs(10),
        connect_timeout: Duration::from_secs(2),
        retry: RetryPolicy { max_retries: 8, base: Duration::from_millis(20), ..Default::default() },
        breaker: BreakerConfig::default(),
        http2_prior_knowledge: false,
        pool_idle_timeout: Duration::from_secs(90),
        dns_cache: false,
    }
}

async fn engine_run(p: Arc<Provider>, n: usize, offered: usize) -> (Vec<f64>, u64, Duration) {
    let t = Instant::now();
    let res: Vec<_> = stream::iter(0..n)
        .map(|i| {
            let p = p.clone();
            async move {
                let s = Instant::now();
                let r = p
                    .call(
                        CallRequest::post("/v1/echo", json!({"n": i}))
                            .idempotency_key(format!("b{i}"))
                            .deadline(Duration::from_secs(60)),
                    )
                    .await;
                (r.is_ok(), s.elapsed().as_secs_f64() * 1000.0)
            }
        })
        .buffer_unordered(offered)
        .collect()
        .await;
    let ok = res.iter().filter(|r| r.0).count() as u64;
    (res.into_iter().filter(|r| r.0).map(|r| r.1).collect(), ok, t.elapsed())
}

/// Naive client: fixed concurrency, immediate retry on any non-2xx up to `retries` times.
async fn naive_run(url: &str, n: usize, offered: usize, retries: u32, keepalive: bool) -> (Vec<f64>, u64, Duration) {
    let mut b = reqwest::Client::builder();
    if !keepalive {
        b = b.pool_max_idle_per_host(0);
    }
    let client = b.build().unwrap_or_default();
    let t = Instant::now();
    let res: Vec<_> = stream::iter(0..n)
        .map(|i| {
            let (client, url) = (client.clone(), format!("{url}/v1/echo"));
            async move {
                let s = Instant::now();
                for _ in 0..=retries {
                    if let Ok(r) = client.post(&url).json(&json!({"n": i})).send().await
                        && r.status().is_success()
                    {
                        return (true, s.elapsed().as_secs_f64() * 1000.0);
                    }
                }
                (false, s.elapsed().as_secs_f64() * 1000.0)
            }
        })
        .buffer_unordered(offered)
        .collect()
        .await;
    let ok = res.iter().filter(|r| r.0).count() as u64;
    (res.into_iter().filter(|r| r.0).map(|r| r.1).collect(), ok, t.elapsed())
}

fn upstream_json(u: &fake_upstream::Upstream) -> Value {
    let s = u.stats();
    json!({"sent": s.requests, "rejected_429": s.rate_limited, "rejected_503": s.unavailable, "errors_500": s.errors,
           "connections": s.connections, "waste_ratio": (s.rate_limited + s.unavailable) as f64 / s.requests.max(1) as f64,
           "connection_reuse": 1.0 - s.connections as f64 / s.requests.max(1) as f64})
}

async fn outbound(smoke: bool) -> anyhow::Result<Value> {
    let scale = if smoke { 1 } else { 5 };
    let up = |b: Behaviour| async move { start(SocketAddr::from(([127, 0, 0, 1], 0)), b).await };
    let mut out = serde_json::Map::new();

    // 1. Connection reuse: pooled keep-alive vs a fresh TCP connection per request.
    {
        let n = 2000 * scale;
        let a = up(Behaviour::default()).await?;
        let p = Arc::new(Provider::new(
            "p",
            settings(&a.url(), AdaptiveConfig { min: 64, max: 64, initial: 64, ..Default::default() }),
        )?);
        let (l, ok, w) = engine_run(p, n, 64).await;
        let mut pooled = summarize(l, ok, n as u64, w);
        pooled["upstream"] = upstream_json(&a.upstream);
        let b = up(Behaviour::default()).await?;
        let (l, ok, w) = naive_run(&b.url(), n, 64, 0, false).await;
        let mut fresh = summarize(l, ok, n as u64, w);
        fresh["upstream"] = upstream_json(&b.upstream);
        out.insert("connection_reuse".into(), json!({"pooled_keepalive": pooled, "fresh_connections": fresh}));
    }

    // 2. Concurrency sweep against 5ms provider latency (Little's law: rps ≈ c / latency).
    {
        let mut sweep = Vec::new();
        for c in [1usize, 8, 32, 128, 512] {
            let n = (c * 40).clamp(400, 20_000) * if smoke { 1 } else { 2 };
            let a = up(Behaviour { base_latency_ms: 5, ..Default::default() }).await?;
            let p = Arc::new(Provider::new(
                "p",
                settings(
                    &a.url(),
                    AdaptiveConfig { min: c, max: c, initial: c, max_queue: 100_000, ..Default::default() },
                ),
            )?);
            let (l, ok, w) = engine_run(p, n, c).await;
            let mut s = summarize(l, ok, n as u64, w);
            s["concurrency"] = json!(c);
            sweep.push(s);
        }
        out.insert("concurrency_sweep_5ms".into(), json!(sweep));
    }

    // 3. Rate-limited provider (500 rps, burst 50, 429 + Retry-After: 1): engine vs naive.
    {
        let n = 1500 * scale;
        let beh = Behaviour { rps: 500.0, burst: 50.0, retry_after_secs: 1, ..Default::default() };
        let a = up(beh.clone()).await?;
        let p = Arc::new(Provider::new("p", settings(&a.url(), AdaptiveConfig::default()))?);
        let (l, ok, w) = engine_run(p, n, 128).await;
        let mut eng = summarize(l, ok, n as u64, w);
        eng["upstream"] = upstream_json(&a.upstream);
        let b = up(beh).await?;
        let (l, ok, w) = naive_run(&b.url(), n, 128, 20, true).await;
        let mut naive = summarize(l, ok, n as u64, w);
        naive["upstream"] = upstream_json(&b.upstream);
        out.insert("rate_limited_500rps".into(), json!({"engine": eng, "naive": naive}));
    }

    // 4. Overloaded provider (capacity 16 at 10ms, hard limit 48 → 503) with 256 offered.
    {
        let n = 3000 * scale;
        let beh = Behaviour { capacity: 16, hard_limit: 48, base_latency_ms: 10, ..Default::default() };
        let a = up(beh.clone()).await?;
        let p = Arc::new(Provider::new("p", settings(&a.url(), AdaptiveConfig { initial: 32, ..Default::default() }))?);
        let (l, ok, w) = engine_run(p.clone(), n, 256).await;
        let mut eng = summarize(l, ok, n as u64, w);
        eng["upstream"] = upstream_json(&a.upstream);
        eng["final_concurrency_limit"] = json!(p.limiter().limit());
        let b = up(beh).await?;
        let (l, ok, w) = naive_run(&b.url(), n, 256, 20, true).await;
        let mut naive = summarize(l, ok, n as u64, w);
        naive["upstream"] = upstream_json(&b.upstream);
        out.insert("overloaded_capacity16".into(), json!({"engine": eng, "naive": naive}));
    }

    // 5. Open-loop arrivals at 400 rps for 6s; provider returns 503 from t=1s to t=3s.
    //    Measures load thrown at a dead provider and time to recover (first success after t=3s).
    for name in ["engine", "naive"] {
        out.insert(format!("outage_open_loop_{name}"), outage_open_loop(name == "engine").await?);
    }
    Ok(Value::Object(out))
}

async fn outage_open_loop(use_engine: bool) -> anyhow::Result<Value> {
    let a =
        start(SocketAddr::from(([127, 0, 0, 1], 0)), Behaviour { base_latency_ms: 2, ..Default::default() }).await?;
    let url = a.url();
    let mut s = settings(&url, AdaptiveConfig::default());
    s.breaker.open_for = Duration::from_millis(500);
    let engine = Arc::new(Provider::new("p", s)?);
    let naive = reqwest::Client::new();
    let (rate, secs, outage) = (400u64, 6u64, (Duration::from_secs(1), Duration::from_secs(3)));
    let t0 = Instant::now();
    let ctl = a.upstream.clone();
    let switcher = tokio::spawn(async move {
        tokio::time::sleep(outage.0).await;
        ctl.set(Behaviour { base_latency_ms: 2, outage: true, ..Default::default() });
        tokio::time::sleep(outage.1 - outage.0).await;
        ctl.set(Behaviour { base_latency_ms: 2, ..Default::default() });
    });
    let mut tick = tokio::time::interval(Duration::from_micros(1_000_000 / rate));
    let mut handles = Vec::new();
    let mut sent_before_outage_end = 0;
    for i in 0..rate * secs {
        tick.tick().await;
        if t0.elapsed() >= outage.1 && sent_before_outage_end == 0 {
            sent_before_outage_end = a.upstream.stats().requests;
        }
        let (engine, naive, url) = (engine.clone(), naive.clone(), url.clone());
        handles.push(tokio::spawn(async move {
            let ok = if use_engine {
                engine
                    .call(CallRequest::post("/v1/echo", json!({"n": i})).deadline(Duration::from_secs(2)))
                    .await
                    .is_ok()
            } else {
                let mut ok = false;
                for _ in 0..=20 {
                    if let Ok(r) = naive.post(format!("{url}/v1/echo")).json(&json!({"n": i})).send().await
                        && r.status().is_success()
                    {
                        ok = true;
                        break;
                    }
                }
                ok
            };
            (ok, t0.elapsed())
        }));
    }
    let mut results = Vec::new();
    for h in handles {
        results.push(h.await?);
    }
    switcher.await?;
    let ok = results.iter().filter(|r| r.0).count();
    let first_after = results.iter().filter(|r| r.0 && r.1 >= outage.1).map(|r| r.1).min();
    let offered_during_outage = (rate as f64 * (outage.1 - outage.0).as_secs_f64()) as u64;
    let st = a.upstream.stats();
    Ok(json!({
        "offered_rps": rate, "duration_s": secs, "outage_window_s": [outage.0.as_secs_f64(), outage.1.as_secs_f64()],
        "offered": results.len(), "succeeded": ok,
        "upstream_requests_until_recovery": sent_before_outage_end,
        "upstream_rejected_503": st.unavailable,
        "amplification_during_outage": st.unavailable as f64 / offered_during_outage as f64,
        "recovery_s": first_after.map(|t| (t - outage.1).as_secs_f64()),
    }))
}

/// Per-request tracing cost: time to create, enter and close spans shaped like the HTTP request
/// span (with W3C parent extraction), with and without the OpenTelemetry layer.
fn span_cost() -> Value {
    use opentelemetry::trace::TracerProvider as _;
    use tracing_subscriber::layer::SubscriberExt as _;
    const N: u32 = 300_000;
    let run = |label: &str, dispatch: tracing::Dispatch| {
        tracing::dispatcher::with_default(&dispatch, || {
            let headers = {
                let mut h = reqwest::header::HeaderMap::new();
                h.insert(
                    "traceparent",
                    "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
                        .parse()
                        .unwrap_or_else(|_| unreachable!()),
                );
                h
            };
            for _ in 0..N / 10 {
                let s = tracing::info_span!("warmup");
                drop(s.enter());
            }
            let t = Instant::now();
            for _ in 0..N {
                let span = tracing::info_span!(
                    "http",
                    method = "GET",
                    route = "/x",
                    request_id = "r",
                    trace_id = tracing::field::Empty
                );
                app_telemetry::propagation::set_parent_from_headers(&span, &headers);
                if let Some(id) = app_telemetry::propagation::trace_id(&span) {
                    span.record("trace_id", id);
                }
                let _e = span.enter();
                let child = tracing::info_span!("db");
                drop(child.enter());
            }
            let ns = t.elapsed().as_nanos() as f64 / f64::from(N);
            json!({"config": label, "ns_per_request_two_spans": (ns * 10.0).round() / 10.0})
        })
    };
    opentelemetry::global::set_text_map_propagator(opentelemetry_sdk::propagation::TraceContextPropagator::new());
    let plain = tracing::Dispatch::new(tracing_subscriber::registry());
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
    let otel = tracing::Dispatch::new(
        tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(provider.tracer("bench"))),
    );
    let a = run("registry only (trace_propagation = false)", plain);
    let b = run("registry + OpenTelemetry layer, no exporter (default)", otel);
    let _ = provider.shutdown();
    json!({"iterations": N, "results": [a, b], "note": "two spans per request (http + one child), parent extracted from traceparent"})
}

/// Create the benchmark database (name taken from BENCH_DATABASE_URL) if it does not exist.
async fn ensure_db() -> anyhow::Result<Value> {
    let admin = std::env::var("BENCH_PG_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://app:app-dev-only@localhost:55432/app".into());
    let target = std::env::var("BENCH_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://app:app-dev-only@localhost:55432/app_bench".into());
    let name = target.rsplit('/').next().and_then(|n| n.split('?').next()).unwrap_or_default().to_string();
    anyhow::ensure!(
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "invalid database name {name:?}"
    );
    let db = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(&admin).await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
        .bind(&name)
        .fetch_one(&db)
        .await?;
    if !exists {
        // Identifiers cannot be bound; `name` is validated above to [A-Za-z0-9_].
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\""))).execute(&db).await?;
    }
    Ok(json!({"database": name, "created": !exists}))
}

/// Idempotent fixture for authenticated HTTP benchmarks: a user who owns `bench-org` (with
/// 1000 runs) and a session with a known token. Prints the cookie value and CSRF token.
async fn seed_http() -> anyhow::Result<Value> {
    let url = std::env::var("BENCH_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://app:app-dev-only@localhost:55432/app_bench".into());
    let db = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await?;
    app_db::MIGRATOR.run(&db).await?;
    let token = "bench".repeat(9)[..43].to_string(); // shape of a real token (43 url-safe chars)
    let (user, org) = ("00000000-0000-7000-8000-00000000c001", "00000000-0000-7000-8000-00000000c002");
    let mut tx = db.begin().await?;
    sqlx::query("INSERT INTO users (id, identity_provider, external_subject, email, email_verified, display_name) VALUES ($1::uuid, 'bench', 'bench-user', 'bench@example.com', true, 'Bench') ON CONFLICT DO NOTHING")
        .bind(user).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO organizations (id, slug, name, created_by) VALUES ($1::uuid, 'bench-http', 'Bench HTTP', $2::uuid) ON CONFLICT DO NOTHING")
        .bind(org).bind(user).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO organization_memberships (organization_id, user_id, role_id) VALUES ($1::uuid, $2::uuid, '00000000-0000-7000-8000-000000000001') ON CONFLICT DO NOTHING")
        .bind(org).bind(user).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO runs (id, organization_id, owner_id, label, provider, requested, status, created_at) SELECT gen_random_uuid(), $1::uuid, $2::uuid, 'run ' || g, 'simulated', 10, 'completed', now() - make_interval(secs => g) FROM generate_series(1, 1000) g WHERE NOT EXISTS (SELECT 1 FROM runs WHERE organization_id = $1::uuid)")
        .bind(org).bind(user).execute(&mut *tx).await?;
    // Fresh timestamps: no rotation or idle expiry during a run (rotation would replace the cookie).
    sqlx::query("DELETE FROM sessions WHERE token_hash = sha256($1::bytea) OR previous_token_hash = sha256($1::bytea)")
        .bind(token.as_bytes())
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO sessions (id, user_id, token_hash, csrf_token, expires_at, idle_expires_at, auth_time) VALUES (gen_random_uuid(), $2::uuid, sha256($1::bytea), 'bench-csrf', now() + interval '1 day', now() + interval '1 day', now())")
        .bind(token.as_bytes()).bind(user).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(json!({"cookie_value": token, "csrf": "bench-csrf", "org_slug": "bench-http"}))
}

/// Time series of the rate controller against a 500 rps provider (for tuning and docs).
async fn trace_rate() -> anyhow::Result<Value> {
    let a = start(
        SocketAddr::from(([127, 0, 0, 1], 0)),
        Behaviour { rps: 500.0, burst: 50.0, retry_after_secs: 1, ..Default::default() },
    )
    .await?;
    let p = Arc::new(Provider::new("p", settings(&a.url(), AdaptiveConfig::default()))?);
    let (pp, up) = (p.clone(), a.upstream.clone());
    let rows = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = rows.clone();
    let sampler = tokio::spawn(async move {
        let t = Instant::now();
        let (mut last_ok, mut last_429) = (0, 0);
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let (h, s) = (pp.health(), up.stats());
            let row = json!({"t": (t.elapsed().as_secs_f64() * 100.0).round() / 100.0, "cap": h.rate_cap_rps.map(f64::round),
                "ok_rps": (s.ok - last_ok) * 4, "r429": s.rate_limited - last_429, "paused_ms": h.paused_for_ms, "conc": h.concurrency_limit});
            (last_ok, last_429) = (s.ok, s.rate_limited);
            if let Ok(mut r) = sink.lock() {
                r.push(row);
            }
        }
    });
    let (_, ok, w) = engine_run(p, 8000, 128).await;
    sampler.abort();
    let rows: Vec<Value> = rows.lock().map(|r| r.clone()).unwrap_or_default();
    Ok(json!({"succeeded": ok, "wall_s": w.as_secs_f64(), "series": rows}))
}

async fn db(smoke: bool) -> anyhow::Result<Value> {
    let url = std::env::var("BENCH_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://app:app-dev-only@localhost:55432/app_bench".into());
    let dur = Duration::from_secs(if smoke { 2 } else { 8 });
    let setup = sqlx::postgres::PgPoolOptions::new().max_connections(4).connect(&url).await?;
    app_db::MIGRATOR.run(&setup).await?;
    // Seed: one organisation with 50k runs, one session (tables come from migrations).
    // Scoped deletes (TRUNCATE ... CASCADE would also empty `roles`, which references organizations).
    sqlx::query("DELETE FROM organizations WHERE id = '00000000-0000-7000-8000-00000000b002'").execute(&setup).await?;
    sqlx::query("DELETE FROM users WHERE id = '00000000-0000-7000-8000-00000000b001'").execute(&setup).await?;
    sqlx::query("INSERT INTO users (id, identity_provider, external_subject, email, display_name) VALUES ('00000000-0000-7000-8000-00000000b001','bench','b','b@bench','B')").execute(&setup).await?;
    sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ('00000000-0000-7000-8000-00000000b002','bench-org','Bench')").execute(&setup).await?;
    sqlx::query("INSERT INTO runs (id, organization_id, owner_id, label, provider, requested, created_at) SELECT gen_random_uuid(), '00000000-0000-7000-8000-00000000b002', '00000000-0000-7000-8000-00000000b001', 'r', 'simulated', 10, now() - make_interval(secs => g) FROM generate_series(1, 50000) g").execute(&setup).await?;
    // 20k sessions (realistic index size); the benchmark looks up one by token hash.
    sqlx::query("INSERT INTO sessions (id, user_id, token_hash, csrf_token, expires_at, idle_expires_at, auth_time) SELECT gen_random_uuid(), '00000000-0000-7000-8000-00000000b001', sha256(g::text::bytea), 'c', now() + interval '1 day', now() + interval '1 day', now() FROM generate_series(1, 20000) g").execute(&setup).await?;
    sqlx::query("ANALYZE").execute(&setup).await?;
    let mut results = Vec::new();
    // `None` = the real per-request session lookup (`app_db::sessions::find_active_with_user`).
    // tenant_list_20 is the SQL of `app_db::runs::list` (first page, no status filter).
    let queries: [(&str, Option<&str>); 4] = [
        // Ids are bound from the client: `WHERE id = random()` is volatile → evaluated per row → seq scan.
        ("pk_read", Some("SELECT id, random_number FROM bench_world WHERE id = $1")),
        (
            "tenant_list_20",
            Some(
                "SELECT id, organization_id, owner_id, label, provider, requested, succeeded, failed, status, created_at, updated_at FROM runs WHERE organization_id = '00000000-0000-7000-8000-00000000b002' AND (NULL::text IS NULL OR status = NULL) ORDER BY created_at DESC, id DESC LIMIT 21",
            ),
        ),
        ("session_auth_lookup", None),
        ("update_one", Some("UPDATE bench_world SET random_number = $1 WHERE id = $1")),
    ];
    let token_hash: Vec<u8> = sqlx::query_scalar("SELECT sha256('12345'::bytea)").fetch_one(&setup).await?;
    anyhow::ensure!(
        app_db::sessions::find_active_with_user(&setup, &token_hash).await?.is_some(),
        "seeded session not found"
    );
    let combos: &[(u32, usize)] = if smoke { &[(16, 64)] } else { &[(8, 64), (32, 64), (32, 256), (64, 256)] };
    for (name, sql) in queries {
        for &(pool_size, concurrency) in combos {
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(pool_size)
                .min_connections(pool_size)
                .connect(&url)
                .await?;
            let stop = Instant::now() + dur;
            let mut tasks = Vec::new();
            for _ in 0..concurrency {
                let (pool, token_hash) = (pool.clone(), token_hash.clone());
                tasks.push(tokio::spawn(async move {
                    let (mut lat, mut wait, mut n) = (Vec::new(), Vec::new(), 0u64);
                    let mut seed = 0x9E37_79B9u32 ^ (std::ptr::from_ref(&lat) as usize as u32);
                    while Instant::now() < stop {
                        let t = Instant::now();
                        let Ok(mut conn) = pool.acquire().await else { continue };
                        let acquired = t.elapsed();
                        let ok = match sql {
                            Some(sql) => {
                                seed ^= seed << 13;
                                seed ^= seed >> 17;
                                seed ^= seed << 5;
                                let q = sqlx::query(sql);
                                let q = if sql.contains("$1") { q.bind((seed % 10_000 + 1) as i32) } else { q };
                                q.execute(&mut *conn).await.is_ok()
                            }
                            None => app_db::sessions::find_active_with_user(&mut *conn, &token_hash).await.is_ok(),
                        };
                        if ok {
                            n += 1;
                            lat.push(t.elapsed().as_secs_f64() * 1000.0);
                            wait.push(acquired.as_secs_f64() * 1000.0);
                        }
                    }
                    (lat, wait, n)
                }));
            }
            let (mut lat, mut wait, mut total) = (Vec::new(), Vec::new(), 0u64);
            for t in tasks {
                let (l, w, n) = t.await?;
                lat.extend(l);
                wait.extend(w);
                total += n;
            }
            lat.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            wait.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            results.push(json!({
                "query": name, "pool_size": pool_size, "concurrency": concurrency, "ops": total,
                "ops_per_sec": total as f64 / dur.as_secs_f64(),
                "p50_ms": pct(&lat, 0.5), "p99_ms": pct(&lat, 0.99), "p999_ms": pct(&lat, 0.999),
                "pool_wait_p50_ms": pct(&wait, 0.5), "pool_wait_p99_ms": pct(&wait, 0.99),
            }));
            pool.close().await;
        }
    }
    Ok(json!({"duration_s": dur.as_secs_f64(), "results": results}))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let smoke = args.iter().any(|a| a == "--smoke");
    let v = match args.get(1).map(String::as_str) {
        Some("outbound") => outbound(smoke).await?,
        Some("db") => db(smoke).await?,
        Some("trace-rate") => trace_rate().await?,
        Some("seed-http") => seed_http().await?,
        Some("ensure-db") => ensure_db().await?,
        Some("span-cost") => span_cost(),
        _ => anyhow::bail!("usage: app-bench outbound|db [--smoke]"),
    };
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}
