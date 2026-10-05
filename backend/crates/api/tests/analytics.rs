#![allow(clippy::unwrap_used)]
//! Analytics endpoint: off by default (404), tenant-scoped when ClickHouse is enabled.
//! The ClickHouse part runs when TEST_CLICKHOUSE_URL is set (each test uses its own database).

mod support;

use axum::http::StatusCode;
use sqlx::PgPool;
use support::*;

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn analytics_is_not_found_when_the_module_is_off(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let b = app.login("noanalytics@example.com").await;
    let slug = app.personal_slug(&b).await;
    assert_eq!(app.get(&b, &format!("/api/v1/orgs/{slug}/analytics/runs")).await.status, StatusCode::NOT_FOUND);
}

#[cfg(feature = "clickhouse")]
mod with_clickhouse {
    use std::time::{Duration, Instant};

    use axum::http::StatusCode;
    use serde_json::json;
    use sqlx::PgPool;

    use super::support::*;

    #[sqlx::test(migrator = "app_db::MIGRATOR")]
    async fn run_analytics_are_recorded_and_tenant_scoped(pool: PgPool) {
        let Ok(url) = std::env::var("TEST_CLICKHOUSE_URL") else {
            eprintln!("TEST_CLICKHOUSE_URL not set: skipping");
            return;
        };
        let user = std::env::var("TEST_CLICKHOUSE_USER").unwrap_or_else(|_| "app".into());
        let password = std::env::var("TEST_CLICKHOUSE_PASSWORD").unwrap_or_else(|_| "app-dev-only".into());
        let db = format!("api_{}", uuid::Uuid::now_v7().simple());
        app_analytics::testing::create_database(&url, &user, &password, &db).await.unwrap();

        let app = TestApp::with_config(pool, |c| {
            c.analytics.enabled = true;
            c.analytics.clickhouse_url = url.clone();
            c.analytics.user = user.clone();
            c.analytics.password = app_config::Secret::new(password.clone());
            c.analytics.database = db.clone();
            c.analytics.flush_interval_ms = 50;
            c.providers.definitions.insert("simulated".into(), Default::default());
        })
        .await;
        let a = app.login("analytics-a@example.com").await;
        let b = app.login("analytics-b@example.com").await;
        let (sa, sb) = (app.personal_slug(&a).await, app.personal_slug(&b).await);
        for n in [10, 20] {
            let r = app
                .post(
                    &a,
                    &format!("/api/v1/orgs/{sa}/runs"),
                    json!({"label": "a", "provider": "simulated", "requested": n}),
                )
                .await;
            assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        }
        let r = app
            .post(
                &b,
                &format!("/api/v1/orgs/{sb}/runs"),
                json!({"label": "b", "provider": "simulated", "requested": 5}),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED);

        // Ingestion is asynchronous (batched): wait for the rows to land.
        let deadline = Instant::now() + Duration::from_secs(10);
        let body = loop {
            let r = app.get(&a, &format!("/api/v1/orgs/{sa}/analytics/runs?days=7")).await;
            assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
            if r.body["created"].as_array().is_some_and(|v| !v.is_empty()) || Instant::now() > deadline {
                break r.body;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert_eq!(body["created"][0]["events"], 2, "only organisation A's runs: {body}");
        assert_eq!(body["created"][0]["value"], 30.0);
        let rb = app.get(&b, &format!("/api/v1/orgs/{sb}/analytics/runs?days=7")).await;
        let bc = &rb.body["created"];
        assert!(bc.as_array().unwrap().is_empty() || bc[0]["events"] == 1, "B sees only its own: {}", rb.body);
        // Another tenant's analytics: not found (membership), never a cross-tenant read.
        assert_eq!(app.get(&a, &format!("/api/v1/orgs/{sb}/analytics/runs")).await.status, StatusCode::NOT_FOUND);
        app_analytics::testing::drop_database(&url, &user, &password, &db).await;
    }
}
