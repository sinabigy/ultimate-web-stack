#![allow(clippy::unwrap_used)]
//! API keys and machine-to-machine (service account) authentication.

mod support;

use axum::http::{Method, StatusCode};
use serde_json::json;
use sqlx::PgPool;
use support::*;

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn api_key_lifecycle(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let owner = app.login("dev@k.example").await;
    let org = app.create_org(&owner, "Keys", "keys").await;
    let created = app
        .post(
            &owner,
            &format!("/api/v1/orgs/{org}/api-keys"),
            json!({"name": "ci", "scopes": ["runs:read", "runs:create"]}),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let key = created.body["key"].as_str().unwrap().to_string();
    assert!(key.starts_with("app_live_"));
    // shown once: listing never contains the secret
    let list = app.get(&owner, &format!("/api/v1/orgs/{org}/api-keys")).await;
    assert!(!list.body.to_string().contains(&key));
    // works within scope
    let runs = format!("/api/v1/orgs/{org}/runs");
    assert_eq!(app.bearer(&key, Method::GET, &runs, None).await.status, StatusCode::OK);
    let r = app
        .bearer(&key, Method::POST, &runs, Some(json!({"label": "ci", "provider": "simulated", "requested": 1})))
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    // out of scope
    assert_eq!(
        app.bearer(&key, Method::GET, &format!("/api/v1/orgs/{org}/members"), None).await.status,
        StatusCode::FORBIDDEN
    );
    // not usable for human-only endpoints
    assert_eq!(app.bearer(&key, Method::GET, "/api/v1/account/profile", None).await.code(), "user_session_required");
    // bound to its organisation
    let personal = app.personal_slug(&owner).await;
    assert_eq!(
        app.bearer(&key, Method::GET, &format!("/api/v1/orgs/{personal}/runs"), None).await.status,
        StatusCode::NOT_FOUND
    );
    // wrong secret / garbage
    let mut tampered = key.clone();
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'a' { 'b' } else { 'a' });
    assert_eq!(app.bearer(&tampered, Method::GET, &runs, None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.bearer("app_live_nope", Method::GET, &runs, None).await.status, StatusCode::UNAUTHORIZED);
    // rotation: new key works, old key keeps working until overlap ends
    let id = created.body["id"].as_str().unwrap();
    let rotated = app.post(&owner, &format!("/api/v1/orgs/{org}/api-keys/{id}/rotate"), json!({})).await;
    assert_eq!(rotated.status, StatusCode::CREATED);
    let new_key = rotated.body["key"].as_str().unwrap().to_string();
    assert_eq!(app.bearer(&new_key, Method::GET, &runs, None).await.status, StatusCode::OK);
    assert_eq!(app.bearer(&key, Method::GET, &runs, None).await.status, StatusCode::OK, "overlap");
    sqlx::query("UPDATE api_keys SET expires_at = now() - interval '1 second' WHERE id = $1::uuid")
        .bind(id)
        .execute(&app.pool)
        .await
        .unwrap();
    assert_eq!(app.bearer(&key, Method::GET, &runs, None).await.status, StatusCode::UNAUTHORIZED, "expired");
    // revoke
    let new_id = rotated.body["id"].as_str().unwrap();
    assert_eq!(
        app.delete(&owner, &format!("/api/v1/orgs/{org}/api-keys/{new_id}")).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(app.bearer(&new_key, Method::GET, &runs, None).await.status, StatusCode::UNAUTHORIZED, "revoked");
    for action in ["api_key.created", "api_key.rotated", "api_key.revoked"] {
        assert_eq!(app.audit_count(action).await, 1, "{action}");
    }
    let secrets_in_audit: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE metadata::text LIKE '%' || $1 || '%'")
            .bind(&key[20..])
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(secrets_in_audit, 0, "key secret never logged");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn api_key_bounded_by_creator_current_role(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let owner = app.login("o@k.example").await;
    let admin = app.login("a@k.example").await;
    let org = app.create_org(&owner, "Bound", "bound").await;
    app.add_member(&owner, &org, &admin, "admin").await;
    let k =
        app.post(&admin, &format!("/api/v1/orgs/{org}/api-keys"), json!({"name": "k", "scopes": ["runs:read"]})).await;
    let key = k.body["key"].as_str().unwrap().to_string();
    let runs = format!("/api/v1/orgs/{org}/runs");
    assert_eq!(app.bearer(&key, Method::GET, &runs, None).await.status, StatusCode::OK);
    // creator removed from the organisation → key stops working
    assert_eq!(
        app.delete(&owner, &format!("/api/v1/orgs/{org}/members/{}", admin.user_id)).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(app.bearer(&key, Method::GET, &runs, None).await.status, StatusCode::NOT_FOUND);
    // scope escalation at creation
    let viewer = app.login("v@k.example").await;
    app.add_member(&owner, &org, &viewer, "viewer").await;
    let r = app
        .post(&viewer, &format!("/api/v1/orgs/{org}/api-keys"), json!({"name": "x", "scopes": ["runs:create"]}))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = app
        .post(&owner, &format!("/api/v1/orgs/{org}/api-keys"), json!({"name": "x", "scopes": ["members:remove"]}))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "membership administration is never credential-assignable");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn service_accounts_use_verified_jwts_and_are_not_users(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let owner = app.login("o@svc.example").await;
    let org = app.create_org(&owner, "Svc", "svc").await;
    let r = app
        .post(
            &owner,
            &format!("/api/v1/orgs/{org}/service-clients"),
            json!({"subject": "svc-reporting", "name": "Reporting", "scopes": ["runs:read"]}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    let runs = format!("/api/v1/orgs/{org}/runs");

    let token = app.service_token("svc-reporting", "runs:read runs:create").await;
    assert_eq!(app.bearer(&token, Method::GET, &runs, None).await.status, StatusCode::OK);
    // registered scopes bound the token's scopes
    let r = app
        .bearer(&token, Method::POST, &runs, Some(json!({"label": "x", "provider": "simulated", "requested": 1})))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // not a user
    assert_eq!(app.bearer(&token, Method::GET, "/api/v1/dashboard", None).await.code(), "user_session_required");
    assert_eq!(app.bearer(&token, Method::GET, "/api/v1/admin/system", None).await.status, StatusCode::FORBIDDEN);
    // other organisations
    let personal = app.personal_slug(&owner).await;
    assert_eq!(
        app.bearer(&token, Method::GET, &format!("/api/v1/orgs/{personal}/runs"), None).await.status,
        StatusCode::NOT_FOUND
    );
    // invalid tokens
    for fault in ["wrong_audience", "wrong_issuer", "expired", "bad_signature"] {
        app.fault(fault).await;
        let bad = app.service_token("svc-reporting", "runs:read").await;
        assert_eq!(app.bearer(&bad, Method::GET, &runs, None).await.status, StatusCode::UNAUTHORIZED, "{fault}");
    }
    // A token carrying only IdP scopes (as ZITADEL issues) gets exactly the registered grant.
    let idp_only = app.service_token("svc-reporting", "openid").await;
    assert_eq!(app.bearer(&idp_only, Method::GET, &runs, None).await.status, StatusCode::OK);
    let r = app
        .bearer(&idp_only, Method::POST, &runs, Some(json!({"label": "x", "provider": "simulated", "requested": 1})))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "registration bounds IdP-scoped tokens");
    // valid token, but client not registered with the application
    let unregistered = app.service_token("svc-unregistered", "runs:read").await;
    assert_eq!(app.bearer(&unregistered, Method::GET, &runs, None).await.status, StatusCode::UNAUTHORIZED);
    // alg=none / HS256 tokens are refused outright
    let none = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJzdWIiOiJzdmMtcmVwb3J0aW5nIn0."; // gitleaks:allow (unsigned test token)
    assert_eq!(app.bearer(none, Method::GET, &runs, None).await.status, StatusCode::UNAUTHORIZED);
}
