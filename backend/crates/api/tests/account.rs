#![allow(clippy::unwrap_used)]
//! Account center: profile, sessions, deletion workflow, notifications.

mod support;

use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;
use support::*;

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn profile_and_preferences(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let b = app.login("p@acc.example").await;
    assert_eq!(app.get(&b, "/api/v1/account/profile").await.body["email"], "p@acc.example");
    let r = app.patch(&b, "/api/v1/account/profile", json!({"display_name": "  "})).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = app
        .patch(&b, "/api/v1/account/profile", json!({"display_name": "P", "avatar_url": "javascript:alert(1)"}))
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "only https avatars");
    let r = app
        .call(Some(&b), axum::http::Method::PUT, "/api/v1/account/preferences", Some(json!({"theme": "dark"})))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(app.get(&b, "/api/v1/account/profile").await.body["preferences"]["theme"], "dark");
    let sec = app.get(&b, "/api/v1/account/security").await;
    assert_eq!(sec.status, StatusCode::OK);
    assert!(sec.body["identity_provider"].is_object());
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn session_listing_marks_current(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let a = app.login("s@acc.example").await;
    let _b = app.login("s@acc.example").await;
    let list = app.get(&a, "/api/v1/account/sessions").await;
    let items = list.body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items.iter().filter(|i| i["current"] == true).count(), 1);
    assert!(items[0]["user_agent"].is_string() || items[0]["user_agent"].is_null());
    // cannot revoke someone else's session
    let other = app.login("x@acc.example").await;
    let sid = items[0]["id"].as_str().unwrap();
    assert_eq!(app.delete(&other, &format!("/api/v1/account/sessions/{sid}")).await.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn account_deletion_workflow(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let b = app.login("del@acc.example").await;
    let other = app.login("other@acc.example").await;
    let org = app.create_org(&b, "Shared", "shared-del").await;
    app.add_member(&b, &org, &other, "member").await;
    // wrong confirmation
    assert_eq!(
        app.post(&b, "/api/v1/account/delete", json!({"confirm_email": "nope@x"})).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // stale authentication requires step-up
    sqlx::query("UPDATE sessions SET auth_time = now() - interval '1 hour' WHERE user_id = $1")
        .bind(b.user_id)
        .execute(&app.pool)
        .await
        .unwrap();
    let r = app.post(&b, "/api/v1/account/delete", json!({"confirm_email": "del@acc.example"})).await;
    assert_eq!((r.status, r.code()), (StatusCode::FORBIDDEN, "reauth_required"));
    // fresh login, but sole owner of a shared organisation → blocked by policy
    let b = app.login("del@acc.example").await;
    let r = app.post(&b, "/api/v1/account/delete", json!({"confirm_email": "del@acc.example"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.body["detail"].as_str().unwrap().contains("shared-del"));
    // transfer ownership then delete
    let promote = app
        .patch(
            &b,
            &format!("/api/v1/orgs/{org}/members/{}", other.user_id),
            json!({"role_id": builtin_role_id("owner")}),
        )
        .await;
    assert_eq!(promote.status, StatusCode::NO_CONTENT);
    let r = app.post(&b, "/api/v1/account/delete", json!({"confirm_email": "DEL@acc.example"})).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
    assert_eq!(app.get(&b, "/api/v1/dashboard").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.audit_count("user.deleted").await, 1);
    let email: String =
        sqlx::query_scalar("SELECT email FROM users WHERE id = $1").bind(b.user_id).fetch_one(&app.pool).await.unwrap();
    assert!(!email.contains("del@acc.example"), "PII scrubbed");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn notifications_are_private(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let a = app.login("a@n.example").await;
    let b = app.login("b@n.example").await;
    let id = app_db::notifications::create(&app.pool, a.user_id, None, "info", "Hello", "", None).await.unwrap();
    assert_eq!(app.get(&a, "/api/v1/notifications").await.body["unread"], 1);
    assert_eq!(
        app.post(&b, &format!("/api/v1/notifications/{id}/read"), json!({})).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.post(&a, &format!("/api/v1/notifications/{id}/read"), json!({})).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(app.get(&a, "/api/v1/notifications").await.body["unread"], 0);
    let d = app.get(&a, "/api/v1/dashboard").await;
    assert!(d.body["widgets"]["account"].is_object() && d.body["widgets"]["security"].is_object());
}
