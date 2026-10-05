#![allow(clippy::unwrap_used)]
//! The authorization boundaries every generated application must keep, asserted through the
//! real router, middleware and PostgreSQL. Frontend route hiding plays no part: these are HTTP
//! requests without a browser. Each row is one boundary:
//!
//! | actor                | target                         | expected |
//! |----------------------|--------------------------------|----------|
//! | anonymous            | protected resource             | 401      |
//! | user                 | own organisation's resource    | allowed  |
//! | user                 | system admin resource          | 403      |
//! | tenant A user        | tenant B data                  | 404      |
//! | organisation admin   | own organisation administration| allowed  |
//! | organisation admin   | system admin action            | 403      |
//! | system admin (MFA)   | system operation               | allowed  |

mod support;

use axum::http::{Method, StatusCode};
use serde_json::json;
use sqlx::PgPool;
use support::*;

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn authorization_matrix(pool: PgPool) {
    matrix(pool, app_config::AuthorizationEngine::Rbac).await;
}

/// The same boundaries with Cedar as the engine: both engines must give identical HTTP outcomes.
#[cfg(feature = "cedar")]
#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn authorization_matrix_cedar(pool: PgPool) {
    matrix(pool, app_config::AuthorizationEngine::Cedar).await;
}

/// A project Cedar policy is enforced on reads as well as writes, through the HTTP API.
#[cfg(feature = "cedar")]
#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn cedar_project_policy_forbids_a_read_over_http(pool: PgPool) {
    let dir = std::env::temp_dir().join(format!("cedar-api-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    // Project rule: whoever cannot invite (i.e. plain members) may not read runs.
    std::fs::write(
        dir.join("project.cedar"),
        r#"@reason("members_no_runs") forbid (principal, action == Action::"runs:read", resource) when { !principal.permissions.contains("members:invite") };"#,
    )
    .unwrap();
    let policy_dir = dir.clone();
    let app = TestApp::with_config(pool, move |c| {
        c.authorization.engine = app_config::AuthorizationEngine::Cedar;
        c.authorization.cedar_policy_dir = policy_dir;
    })
    .await;
    let owner = app.login("owner@cedar-read.example").await;
    let org = app.create_org(&owner, "Cedar Read", "cedar-read").await;
    let member = app.login("member@cedar-read.example").await;
    app.add_member(&owner, &org, &member, "member").await;

    assert_eq!(app.get(&owner, &format!("/api/v1/orgs/{org}/runs")).await.status, StatusCode::OK);
    let denied = app.get(&member, &format!("/api/v1/orgs/{org}/runs")).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "project policy applies to reads: {:?}", denied.body);
    // Other reads are unaffected.
    assert_eq!(app.get(&member, &format!("/api/v1/orgs/{org}/members")).await.status, StatusCode::OK);
    std::fs::remove_dir_all(dir).ok();
}

async fn matrix(pool: PgPool, engine: app_config::AuthorizationEngine) {
    let app = TestApp::with_config(pool, |c| {
        c.authorization.engine = engine;
        c.providers.definitions.insert("simulated".into(), Default::default());
    })
    .await;

    // Tenants and actors.
    let owner_a = app.login("owner-a@matrix.example").await;
    let tenant_a = app.create_org(&owner_a, "Tenant A", "tenant-a").await;
    let member_a = app.login("member-a@matrix.example").await;
    app.add_member(&owner_a, &tenant_a, &member_a, "member").await;
    let admin_a = app.login("admin-a@matrix.example").await;
    app.add_member(&owner_a, &tenant_a, &admin_a, "admin").await;
    let owner_b = app.login("owner-b@matrix.example").await;
    let tenant_b = app.create_org(&owner_b, "Tenant B", "tenant-b").await;
    let run_b = app
        .post(
            &owner_b,
            &format!("/api/v1/orgs/{tenant_b}/runs"),
            json!({"label": "b", "provider": "simulated", "requested": 1}),
        )
        .await;
    assert_eq!(run_b.status, StatusCode::CREATED, "{:?}", run_b.body);
    let run_b_id = run_b.body["id"].as_str().unwrap().to_string();

    // 1. anonymous → protected resource = denied (401), for every kind of protected route.
    for path in [
        "/api/v1/account/profile".to_string(),
        "/api/v1/dashboard".to_string(),
        format!("/api/v1/orgs/{tenant_a}/runs"),
        "/api/v1/admin/users".to_string(),
    ] {
        assert_eq!(app.call(None, Method::GET, &path, None).await.status, StatusCode::UNAUTHORIZED, "anonymous {path}");
    }
    let anon_write = app
        .call(
            None,
            Method::POST,
            &format!("/api/v1/orgs/{tenant_a}/runs"),
            Some(json!({"label": "x", "provider": "simulated", "requested": 1})),
        )
        .await;
    assert_eq!(anon_write.status, StatusCode::UNAUTHORIZED, "anonymous write");

    // 2. user → own allowed resource = allowed.
    assert_eq!(app.get(&member_a, "/api/v1/account/profile").await.status, StatusCode::OK);
    assert_eq!(app.get(&member_a, &format!("/api/v1/orgs/{tenant_a}/runs")).await.status, StatusCode::OK);
    let own = app
        .post(
            &member_a,
            &format!("/api/v1/orgs/{tenant_a}/runs"),
            json!({"label": "mine", "provider": "simulated", "requested": 1}),
        )
        .await;
    assert_eq!(own.status, StatusCode::CREATED, "members create runs in their own organisation");

    // 3. user → admin resource = denied (403), both organisation and system administration.
    assert_eq!(app.get(&member_a, "/api/v1/admin/users").await.status, StatusCode::FORBIDDEN, "system admin");
    let r = app.patch(&member_a, &format!("/api/v1/orgs/{tenant_a}"), json!({"name": "Hijacked"})).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "member cannot administer the organisation");

    // 4. tenant A user → tenant B data = denied (404: existence is not revealed).
    for path in [
        format!("/api/v1/orgs/{tenant_b}"),
        format!("/api/v1/orgs/{tenant_b}/runs"),
        format!("/api/v1/orgs/{tenant_b}/runs/{run_b_id}"),
        format!("/api/v1/orgs/{tenant_b}/members"),
        format!("/api/v1/orgs/{tenant_b}/audit"),
    ] {
        assert_eq!(app.get(&owner_a, &path).await.status, StatusCode::NOT_FOUND, "cross-tenant {path}");
    }
    // ...including by guessing a resource id under the attacker's own tenant.
    assert_eq!(
        app.get(&owner_a, &format!("/api/v1/orgs/{tenant_a}/runs/{run_b_id}")).await.status,
        StatusCode::NOT_FOUND,
        "tenant B's run id under tenant A"
    );
    assert_eq!(
        app.delete(&owner_a, &format!("/api/v1/orgs/{tenant_a}/runs/{run_b_id}")).await.status,
        StatusCode::NOT_FOUND,
        "cannot delete another tenant's run by id"
    );

    // 5. organisation admin → own organisation administration = allowed.
    let r = app.patch(&admin_a, &format!("/api/v1/orgs/{tenant_a}"), json!({"name": "Tenant A Renamed"})).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(app.get(&admin_a, &format!("/api/v1/orgs/{tenant_a}/members")).await.status, StatusCode::OK);
    assert_eq!(app.get(&admin_a, &format!("/api/v1/orgs/{tenant_a}/audit")).await.status, StatusCode::OK);

    // 6. organisation admin (and owner) → system admin action = denied.
    for b in [&admin_a, &owner_a] {
        assert_eq!(app.get(b, "/api/v1/admin/users").await.status, StatusCode::FORBIDDEN);
        let r =
            app.patch(b, &format!("/api/v1/admin/users/{}", member_a.user_id), json!({"status": "suspended"})).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "organisation roles never grant platform powers");
    }
    assert_eq!(app.get(&member_a, "/api/v1/dashboard").await.status, StatusCode::OK, "member was not suspended");

    // 7. system admin (MFA session) → system operation = allowed.
    let root = app.try_login("root@matrix.example", "mfa", "").await.unwrap();
    app.set_system_role(root.user_id, "system_admin").await;
    assert_eq!(app.get(&root, "/api/v1/admin/users").await.status, StatusCode::OK);
    let r =
        app.patch(&root, &format!("/api/v1/admin/users/{}", member_a.user_id), json!({"status": "suspended"})).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(
        app.get(&member_a, "/api/v1/dashboard").await.status,
        StatusCode::UNAUTHORIZED,
        "suspension took effect"
    );

    // Every denial above that touched an organisation or the platform was audited.
    let (org_denied, authz_denied, admin_denied) = (
        app.audit_count("organization.access_denied").await,
        app.audit_count("authz.denied").await,
        app.audit_count("admin.access_denied").await,
    );
    eprintln!("audited denials: organization={org_denied} authz={authz_denied} admin={admin_denied}");
    assert!(org_denied >= 5, "every cross-tenant probe is audited ({org_denied})");
    assert!(authz_denied >= 1, "the member's forbidden organisation update is audited ({authz_denied})");
    assert!(admin_denied >= 5, "every denied platform action is audited ({admin_denied})");
}
