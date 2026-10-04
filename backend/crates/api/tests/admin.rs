#![allow(clippy::unwrap_used)]
//! System-administration trust boundary.

mod support;

use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;
use support::*;

const ADMIN_PATHS: &[&str] = &[
    "/api/v1/admin/overview",
    "/api/v1/admin/users",
    "/api/v1/admin/organizations",
    "/api/v1/admin/roles",
    "/api/v1/admin/audit",
    "/api/v1/admin/jobs",
    "/api/v1/admin/providers",
    "/api/v1/admin/system",
];

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn ordinary_users_and_org_owners_are_denied(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let owner = app.login("owner@t.example").await; // owns their personal org + one more
    app.create_org(&owner, "Big Tenant", "big-tenant").await;
    for p in ADMIN_PATHS {
        let r = app.get(&owner, p).await;
        assert_eq!((r.status, r.code()), (StatusCode::FORBIDDEN, "system_privilege_required"), "{p}");
    }
    let target = app.login("victim@t.example").await;
    let r = app.patch(&owner, &format!("/api/v1/admin/users/{}", target.user_id), json!({"status": "suspended"})).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(app.audit_count("admin.access_denied").await >= 9, "every attempt is audited");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn system_admin_requires_mfa_when_configured(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let pwd = app.login("root@t.example").await;
    app.set_system_role(pwd.user_id, "system_admin").await;
    let r = app.get(&pwd, "/api/v1/admin/users").await;
    assert_eq!((r.status, r.code()), (StatusCode::FORBIDDEN, "mfa_required"));
    let mfa = app.try_login("root@t.example", "mfa", "").await.unwrap();
    for p in ADMIN_PATHS {
        assert_eq!(app.get(&mfa, p).await.status, StatusCode::OK, "{p}");
    }
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn auditor_reads_but_cannot_manage(pool: PgPool) {
    let app = TestApp::with_config(pool, |c| c.auth.require_mfa_for_system_admin = false).await;
    let aud = app.login("aud@t.example").await;
    app.set_system_role(aud.user_id, "system_auditor").await;
    let victim = app.login("v@t.example").await;
    assert_eq!(app.get(&aud, "/api/v1/admin/users").await.status, StatusCode::OK);
    let r = app.patch(&aud, &format!("/api/v1/admin/users/{}", victim.user_id), json!({"status": "suspended"})).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(
        app.post(&aud, &format!("/api/v1/admin/users/{}/revoke-sessions", victim.user_id), json!({})).await.status,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn mutating_admin_actions_require_recent_authentication(pool: PgPool) {
    let app = TestApp::with_config(pool, |c| c.auth.require_mfa_for_system_admin = false).await;
    let root = app.login("root@r.example").await;
    app.set_system_role(root.user_id, "system_admin").await;
    let user = app.login("u@r.example").await;
    // Session is valid, but the login was an hour ago.
    sqlx::query("UPDATE sessions SET auth_time = now() - interval '1 hour' WHERE user_id = $1")
        .bind(root.user_id)
        .execute(&app.pool)
        .await
        .unwrap();
    assert_eq!(app.get(&root, "/api/v1/admin/users").await.status, StatusCode::OK, "reads need no step-up");
    let r = app.patch(&root, &format!("/api/v1/admin/users/{}", user.user_id), json!({"status": "suspended"})).await;
    assert_eq!(r.code(), "reauth_required");
    let r = app.post(&root, &format!("/api/v1/admin/users/{}/revoke-sessions", user.user_id), json!({})).await;
    assert_eq!(r.code(), "reauth_required");
    assert_eq!(app.get(&user, "/api/v1/dashboard").await.status, StatusCode::OK, "nothing changed");
    assert!(app.audit_count("admin.access_denied").await >= 2);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn admin_actions_take_effect_and_are_audited(pool: PgPool) {
    let app = TestApp::with_config(pool, |c| c.auth.require_mfa_for_system_admin = false).await;
    let root = app.login("root@t.example").await;
    app.set_system_role(root.user_id, "system_admin").await;
    let user = app.login("u@t.example").await;
    // cannot modify self
    assert_eq!(
        app.patch(&root, &format!("/api/v1/admin/users/{}", root.user_id), json!({"status": "suspended"})).await.code(),
        "cannot_modify_self"
    );
    // suspend ends sessions immediately
    let r = app.patch(&root, &format!("/api/v1/admin/users/{}", user.user_id), json!({"status": "suspended"})).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(app.get(&user, "/api/v1/dashboard").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.audit_count("admin.user_updated").await, 1);
    // granting system roles revokes the target's sessions (privilege change → re-auth)
    let other = app.login("o@t.example").await;
    let r = app
        .patch(&root, &format!("/api/v1/admin/users/{}", other.user_id), json!({"system_role": "system_auditor"}))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(app.get(&other, "/api/v1/dashboard").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.audit_count("role.system_assigned").await, 1);
    // last system admin cannot be demoted (by another admin path); simulate with a second admin
    let second = app.login("second@t.example").await;
    app.set_system_role(second.user_id, "system_admin").await;
    let r = app.patch(&second, &format!("/api/v1/admin/users/{}", root.user_id), json!({"system_role": "none"})).await;
    assert_eq!(r.status, StatusCode::OK, "two admins: demotion allowed");
    let r = app.patch(&root, &format!("/api/v1/admin/users/{}", second.user_id), json!({"system_role": "none"})).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "root's sessions were revoked by the demotion");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn system_roles_from_identity_provider(pool: PgPool) {
    let app = TestApp::with_config(pool, |c| {
        c.auth.system_roles_from_idp = true;
        c.auth.require_mfa_for_system_admin = false;
    })
    .await;
    let b = app.try_login("ops@t.example", "pwd", "system_admin").await.unwrap();
    assert_eq!(app.get(&b, "/api/v1/admin/system").await.status, StatusCode::OK);
    assert_eq!(app.audit_count("role.system_synced").await, 1);
    // role removed at the IdP → downgraded at next login
    let b = app.try_login("ops@t.example", "pwd", "").await.unwrap();
    assert_eq!(app.get(&b, "/api/v1/admin/system").await.status, StatusCode::FORBIDDEN);
    // and the admin API refuses to edit IdP-managed system roles
    let root = app.try_login("root2@t.example", "pwd", "system_admin").await.unwrap();
    let r =
        app.patch(&root, &format!("/api/v1/admin/users/{}", b.user_id), json!({"system_role": "system_admin"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn bootstrap_admin_requires_verified_email(pool: PgPool) {
    let app = TestApp::with_config(pool, |c| {
        c.auth.bootstrap_system_admins = vec!["boot@t.example".into()];
        c.auth.require_mfa_for_system_admin = false;
    })
    .await;
    let b = app.login("boot@t.example").await;
    assert_eq!(app.get(&b, "/api/v1/admin/overview").await.status, StatusCode::OK);
}
