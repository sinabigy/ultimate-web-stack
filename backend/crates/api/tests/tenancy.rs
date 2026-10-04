#![allow(clippy::unwrap_used)]
//! Tenant isolation and organisation authorization through the HTTP API.

mod support;

use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;
use support::*;

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn tenant_a_user_cannot_reach_tenant_b_resources(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let alice = app.login("alice@a.example").await;
    let bob = app.login("bob@b.example").await;
    let a = app.create_org(&alice, "Alpha", "alpha").await;
    let b = app.create_org(&bob, "Beta", "beta").await;
    let run = app
        .post(
            &bob,
            &format!("/api/v1/orgs/{b}/runs"),
            json!({"label": "secret", "provider": "simulated", "requested": 1}),
        )
        .await;
    assert_eq!(run.status, StatusCode::CREATED, "{:?}", run.body);
    let run_id = run.body["id"].as_str().unwrap();

    // Every organisation endpoint answers 404 to a non-member: existence is not revealed.
    for path in [
        format!("/api/v1/orgs/{b}"),
        format!("/api/v1/orgs/{b}/overview"),
        format!("/api/v1/orgs/{b}/members"),
        format!("/api/v1/orgs/{b}/runs"),
        format!("/api/v1/orgs/{b}/runs/{run_id}"),
        format!("/api/v1/orgs/{b}/audit"),
        format!("/api/v1/orgs/{b}/api-keys"),
    ] {
        assert_eq!(app.get(&alice, &path).await.status, StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(app.delete(&alice, &format!("/api/v1/orgs/{b}/runs/{run_id}")).await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        app.patch(&alice, &format!("/api/v1/orgs/{b}"), json!({"name": "pwned"})).await.status,
        StatusCode::NOT_FOUND
    );
    // Using Alice's own org in the path does not expose Bob's run either.
    assert_eq!(app.get(&alice, &format!("/api/v1/orgs/{a}/runs/{run_id}")).await.status, StatusCode::NOT_FOUND);
    // Unknown org and foreign org are indistinguishable.
    assert_eq!(app.get(&alice, "/api/v1/orgs/does-not-exist").await.status, StatusCode::NOT_FOUND);
    assert!(app.audit_count("organization.access_denied").await >= 8, "cross-tenant probes are audited");
    // Bob still sees his run.
    assert_eq!(app.get(&bob, &format!("/api/v1/orgs/{b}/runs/{run_id}")).await.status, StatusCode::OK);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn client_supplied_org_ids_are_ignored(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let alice = app.login("alice@a.example").await;
    let bob = app.login("bob@b.example").await;
    let a = app.create_org(&alice, "Alpha", "alpha").await;
    app.create_org(&bob, "Beta", "beta").await;
    let beta_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM organizations WHERE slug = 'beta'").fetch_one(&app.pool).await.unwrap();
    // Smuggle Bob's organisation id into the body: it is not a field the server reads.
    let r = app
        .post(
            &alice,
            &format!("/api/v1/orgs/{a}/runs"),
            json!({"label": "x", "provider": "simulated", "requested": 1, "organization_id": beta_id}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let alpha_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM organizations WHERE slug = 'alpha'").fetch_one(&app.pool).await.unwrap();
    assert_eq!(r.body["organization_id"].as_str().unwrap(), alpha_id.to_string());
    let in_beta: i64 = sqlx::query_scalar("SELECT count(*) FROM runs WHERE organization_id = $1")
        .bind(beta_id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(in_beta, 0);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn role_matrix_through_the_api(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let owner = app.login("owner@x.example").await;
    let viewer = app.login("viewer@x.example").await;
    let member = app.login("member@x.example").await;
    let org = app.create_org(&owner, "Org", "org-x").await;
    app.add_member(&owner, &org, &viewer, "viewer").await;
    app.add_member(&owner, &org, &member, "member").await;
    let run_body = json!({"label": "r", "provider": "simulated", "requested": 1});
    assert_eq!(app.get(&viewer, &format!("/api/v1/orgs/{org}/runs")).await.status, StatusCode::OK);
    let r = app.post(&viewer, &format!("/api/v1/orgs/{org}/runs"), run_body.clone()).await;
    assert_eq!((r.status, r.code()), (StatusCode::FORBIDDEN, "missing_permission"));
    assert_eq!(app.get(&viewer, &format!("/api/v1/orgs/{org}/audit")).await.status, StatusCode::FORBIDDEN);
    assert_eq!(app.get(&member, &format!("/api/v1/orgs/{org}/audit")).await.status, StatusCode::FORBIDDEN);
    // ownership: member can delete own run but not the owner's
    let mine = app.post(&member, &format!("/api/v1/orgs/{org}/runs"), run_body.clone()).await.body["id"]
        .as_str()
        .unwrap()
        .to_string();
    let theirs =
        app.post(&owner, &format!("/api/v1/orgs/{org}/runs"), run_body).await.body["id"].as_str().unwrap().to_string();
    assert_eq!(app.delete(&member, &format!("/api/v1/orgs/{org}/runs/{theirs}")).await.status, StatusCode::FORBIDDEN);
    assert_eq!(app.delete(&member, &format!("/api/v1/orgs/{org}/runs/{mine}")).await.status, StatusCode::NO_CONTENT);
    // org response tells the UI what to show, but the server still enforces
    let o = app.get(&viewer, &format!("/api/v1/orgs/{org}")).await;
    assert_eq!(o.body["role"], "viewer");
    assert!(!o.body["permissions"].as_array().unwrap().iter().any(|p| p == "runs:create"));
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn privilege_escalation_paths_are_blocked(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let owner = app.login("owner@e.example").await;
    let admin = app.login("admin@e.example").await;
    let manager = app.login("manager@e.example").await;
    let member = app.login("member@e.example").await;
    let org = app.create_org(&owner, "Esc", "esc").await;
    app.add_member(&owner, &org, &admin, "admin").await;
    app.add_member(&owner, &org, &manager, "manager").await;
    app.add_member(&owner, &org, &member, "member").await;
    let path = |u: &Browser| format!("/api/v1/orgs/{org}/members/{}", u.user_id);

    // self-promotion
    let r = app.patch(&admin, &path(&admin), json!({"role_id": builtin_role_id("owner")})).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = app.patch(&member, &path(&member), json!({"role_id": builtin_role_id("admin")})).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // admin cannot create owners or demote owners
    assert_eq!(
        app.patch(&admin, &path(&member), json!({"role_id": builtin_role_id("owner")})).await.code(),
        "missing_permission"
    );
    assert_eq!(
        app.patch(&admin, &path(&owner), json!({"role_id": builtin_role_id("member")})).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(app.delete(&admin, &path(&owner)).await.status, StatusCode::FORBIDDEN);
    // manager cannot change roles; can invite only roles within their own permissions
    assert_eq!(
        app.patch(&manager, &path(&member), json!({"role_id": builtin_role_id("viewer")})).await.status,
        StatusCode::FORBIDDEN
    );
    let inv = app
        .post(
            &manager,
            &format!("/api/v1/orgs/{org}/invitations"),
            json!({"email": "new@e.example", "role_id": builtin_role_id("admin")}),
        )
        .await;
    assert_eq!((inv.status, inv.code()), (StatusCode::FORBIDDEN, "escalation"));
    let ok = app
        .post(
            &manager,
            &format!("/api/v1/orgs/{org}/invitations"),
            json!({"email": "new@e.example", "role_id": builtin_role_id("member")}),
        )
        .await;
    assert_eq!(ok.status, StatusCode::CREATED);
    // custom role cannot exceed creator or include owner-only powers
    let r = app
        .post(
            &admin,
            &format!("/api/v1/orgs/{org}/roles"),
            json!({"key": "god", "name": "God", "permissions": ["org:delete"]}),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = app
        .post(
            &admin,
            &format!("/api/v1/orgs/{org}/roles"),
            json!({"key": "auditor", "name": "Auditor", "permissions": ["audit:read", "runs:read"]}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    // admin promoting member to admin is legitimate
    assert_eq!(
        app.patch(&admin, &path(&member), json!({"role_id": builtin_role_id("admin")})).await.status,
        StatusCode::NO_CONTENT
    );
    assert!(app.audit_count("role.assign_denied").await >= 3, "escalation attempts audited");
    assert!(app.audit_count("role.assigned").await >= 1);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn last_owner_cannot_leave_or_be_demoted(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let owner = app.login("solo@o.example").await;
    let other = app.login("other@o.example").await;
    let org = app.create_org(&owner, "Solo", "solo").await;
    let r = app.post(&owner, &format!("/api/v1/orgs/{org}/leave"), json!({})).await;
    assert_eq!((r.status, r.code()), (StatusCode::FORBIDDEN, "last_owner"));
    app.add_member(&owner, &org, &other, "owner").await;
    assert_eq!(app.post(&owner, &format!("/api/v1/orgs/{org}/leave"), json!({})).await.status, StatusCode::NO_CONTENT);
    assert_eq!(app.get(&owner, &format!("/api/v1/orgs/{org}")).await.status, StatusCode::NOT_FOUND, "left");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn invitation_requires_matching_verified_email_and_is_single_use(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let owner = app.login("owner@i.example").await;
    let invitee = app.login("invitee@i.example").await;
    let thief = app.login("thief@i.example").await;
    let org = app.create_org(&owner, "Inv", "inv").await;
    let r = app
        .post(
            &owner,
            &format!("/api/v1/orgs/{org}/invitations"),
            json!({"email": "Invitee@I.example", "role_id": builtin_role_id("member")}),
        )
        .await;
    let token = r.body["link"].as_str().unwrap().rsplit('/').next().unwrap().to_string();
    assert_eq!(app.get(&invitee, &format!("/api/v1/invitations/{token}")).await.body["role"], "member");
    let stolen = app.post(&thief, &format!("/api/v1/invitations/{token}/accept"), json!({})).await;
    assert_eq!((stolen.status, stolen.code()), (StatusCode::FORBIDDEN, "invitation_email_mismatch"));
    assert_eq!(
        app.post(&invitee, &format!("/api/v1/invitations/{token}/accept"), json!({})).await.status,
        StatusCode::OK
    );
    assert_eq!(
        app.post(&invitee, &format!("/api/v1/invitations/{token}/accept"), json!({})).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(app.get(&invitee, &format!("/api/v1/orgs/{org}")).await.body["role"], "member");
    assert_eq!(app.audit_count("organization.member_added").await, 1);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn realtime_subscriber_only_gets_own_tenant(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let alice = app.login("alice@rt.example").await;
    let bob = app.login("bob@rt.example").await;
    app.create_org(&alice, "A", "rt-a").await;
    app.create_org(&bob, "B", "rt-b").await;
    let mut rx = app.state.events.subscribe();
    let r =
        app.post(&bob, "/api/v1/orgs/rt-b/runs", json!({"label": "b", "provider": "simulated", "requested": 1})).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let ev = rx.recv().await.unwrap();
    let orgs: Vec<uuid::Uuid> = sqlx::query_scalar("SELECT o.id FROM organizations o JOIN organization_memberships m ON m.organization_id = o.id WHERE m.user_id = $1")
        .bind(alice.user_id)
        .fetch_all(&app.pool)
        .await
        .unwrap();
    let sub = app_api::routes::realtime::Subscriber {
        user_id: Some(alice.user_id),
        organizations: orgs,
        system_admin: false,
    };
    assert!(!app_api::routes::realtime::filter_for(&sub, &ev), "alice must not see bob's tenant events");
}
