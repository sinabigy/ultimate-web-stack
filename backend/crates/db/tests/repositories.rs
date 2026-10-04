#![allow(clippy::unwrap_used)]
//! Repository tests against real PostgreSQL. Each test gets a fresh, migrated database
//! (`#[sqlx::test]`; requires DATABASE_URL pointing at a server where the user may CREATE DATABASE).

use std::time::Duration;

use app_authz::{Actor, OrgAccess, PermissionSet, UserActor, rbac};
use app_db::{
    DbError, audit, jobs,
    orgs::{self},
    runs, sessions,
    users::{self, IdentityClaims},
};
use app_domain::{NewRun, OrgRole, Slug, SystemRole};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

fn claims<'a>(sub: &'a str, email: &'a str) -> IdentityClaims<'a> {
    IdentityClaims {
        issuer: "https://idp.test",
        subject: sub,
        email,
        email_verified: true,
        name: Some("Ada Lovelace"),
        picture: None,
    }
}

async fn user_with_org(pool: &PgPool, sub: &str) -> (Uuid, orgs::OrgRow) {
    let (u, _) = users::upsert_from_login(pool, &claims(sub, &format!("{sub}@example.com")), false).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let org = orgs::ensure_personal_org(&mut tx, u.id, &u.display_name).await.unwrap();
    tx.commit().await.unwrap();
    (u.id, org)
}

fn access(org: Uuid, user: Uuid) -> OrgAccess {
    let actor = Actor::User(UserActor {
        id: user,
        system_role: SystemRole::None,
        active: true,
        mfa: false,
        auth_age: Duration::ZERO,
    });
    OrgAccess::__for_tests(org, actor, PermissionSet::all())
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn statement_timeout_applied_to_pool_connections(_pool: PgPool) {
    let cfg = app_config::DatabaseConfig {
        url: app_config::Secret::new(std::env::var("DATABASE_URL").unwrap()),
        statement_timeout_ms: 1234,
        ..Default::default()
    };
    let pool = app_db::connect(&cfg, "test").await.unwrap();
    let v: String = sqlx::query_scalar("SHOW statement_timeout").fetch_one(&pool).await.unwrap();
    assert_eq!(v, "1234ms");
    app_db::ping(&pool).await.unwrap();
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn login_upsert_keys_on_issuer_and_subject(pool: PgPool) {
    let (a, created) = users::upsert_from_login(&pool, &claims("sub-1", "ada@example.com"), false).await.unwrap();
    assert!(created);
    assert_eq!(a.display_name, "Ada Lovelace");
    users::update_profile(&pool, a.id, "Countess", None).await.unwrap();
    // Same identity, changed email at the IdP: same user, email cache refreshed, app-owned name kept.
    let (b, created) = users::upsert_from_login(&pool, &claims("sub-1", "ada@new.example"), false).await.unwrap();
    assert!(!created);
    assert_eq!((b.id, b.email.as_str(), b.display_name.as_str()), (a.id, "ada@new.example", "Countess"));
    // Same email, different subject: a different user (email is not an identity key).
    let (c, created) = users::upsert_from_login(&pool, &claims("sub-2", "ada@new.example"), false).await.unwrap();
    assert!(created && c.id != a.id);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn bootstrap_admin_upgrades_but_never_downgrades(pool: PgPool) {
    let (u, _) = users::upsert_from_login(&pool, &claims("s", "root@example.com"), false).await.unwrap();
    assert_eq!(u.system_role(), SystemRole::None);
    let (u, _) = users::upsert_from_login(&pool, &claims("s", "root@example.com"), true).await.unwrap();
    assert_eq!(u.system_role(), SystemRole::SystemAdmin);
    users::set_system_role(&pool, u.id, SystemRole::SystemAuditor).await.unwrap();
    let (u, _) = users::upsert_from_login(&pool, &claims("s", "root@example.com"), true).await.unwrap();
    assert_eq!(u.system_role(), SystemRole::SystemAuditor, "explicit admin decision is not overridden");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn personal_org_is_created_once_and_slug_collisions_resolve(pool: PgPool) {
    let (u1, org1) = user_with_org(&pool, "one").await;
    let mut tx = pool.begin().await.unwrap();
    let again = orgs::ensure_personal_org(&mut tx, u1, "Ada Lovelace").await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(org1.id, again.id);
    let (_, org2) = user_with_org(&pool, "two").await; // same display name → same base slug
    assert_ne!(org1.slug, org2.slug);
    assert!(org2.slug.starts_with("ada-lovelace"));
    let facts = orgs::facts_for_user_by_slug(&pool, u1, &org1.slug).await.unwrap().facts;
    assert_eq!(facts.role.unwrap().builtin, Some(OrgRole::Owner));
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn tenant_isolation_at_repository_layer(pool: PgPool) {
    let (ua, org_a) = user_with_org(&pool, "a").await;
    let (ub, org_b) = user_with_org(&pool, "b").await;
    let new = NewRun { label: "x".into(), provider: "simulated".into(), requested: 1 };
    let run_b = runs::create(&pool, &access(org_b.id, ub), Some(ub), &new).await.unwrap();
    let a = access(org_a.id, ua);
    assert!(matches!(runs::get(&pool, &a, run_b.id).await, Err(DbError::NotFound)), "cross-tenant read");
    assert!(matches!(runs::delete(&pool, &a, run_b.id).await, Err(DbError::NotFound)), "cross-tenant delete");
    assert!(runs::list(&pool, &a, None, None, 50).await.unwrap().items.is_empty());
    // Membership facts for B's org, as user A, show no role.
    let facts = orgs::facts_for_user_by_slug(&pool, ua, &org_b.slug).await.unwrap().facts;
    assert!(facts.role.is_none());
    assert!(runs::get(&pool, &access(org_b.id, ub), run_b.id).await.is_ok());
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn audit_events_are_append_only_and_scrubbed(pool: PgPool) {
    let e = audit::AuditEvent::new("user.login", audit::Outcome::Success)
        .meta(serde_json::json!({"method": "passkey", "id_token": "eyJ..."}));
    let id = audit::insert(&pool, &e).await.unwrap();
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT metadata FROM audit_events WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(stored["id_token"], "[redacted]");
    assert_eq!(stored["method"], "passkey");
    assert!(sqlx::query("UPDATE audit_events SET action = 'x'").execute(&pool).await.is_err());
    assert!(sqlx::query("DELETE FROM audit_events").execute(&pool).await.is_err());
    assert!(sqlx::query("TRUNCATE audit_events").execute(&pool).await.is_err());
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn audit_listing_is_scoped_and_paginated(pool: PgPool) {
    let (org_a, org_b) = (Uuid::now_v7(), Uuid::now_v7());
    for i in 0..5 {
        let org = if i % 2 == 0 { org_a } else { org_b };
        let mut e = audit::AuditEvent::new("organization.member_added", audit::Outcome::Success);
        e.organization_id = Some(org);
        audit::insert(&pool, &e).await.unwrap();
    }
    let f = audit::AuditFilter::default();
    let p1 = audit::list(&pool, Some(org_a), &f, None, 2).await.unwrap();
    assert_eq!(p1.items.len(), 2);
    assert!(p1.items.iter().all(|r| r.organization_id == Some(org_a)));
    let cursor = app_db::pagination::Cursor::decode(p1.next_cursor.as_deref().unwrap());
    let p2 = audit::list(&pool, Some(org_a), &f, cursor, 2).await.unwrap();
    assert_eq!(p2.items.len(), 1);
    assert!(p2.next_cursor.is_none());
    assert_eq!(audit::list(&pool, None, &f, None, 50).await.unwrap().items.len(), 5);
}

async fn new_session(pool: &PgPool, user: Uuid, hash: &[u8], expires: OffsetDateTime, idle: OffsetDateTime) -> Uuid {
    let id = Uuid::now_v7();
    sessions::create(
        pool,
        &sessions::NewSession {
            id,
            user_id: user,
            token_hash: hash,
            csrf_token: "csrf",
            expires_at: expires,
            idle_expires_at: idle,
            auth_time: OffsetDateTime::now_utc(),
            amr: &["pwd".into()],
            mfa: false,
            ip: None,
            user_agent: Some("test"),
            id_token_enc: None,
        },
    )
    .await
    .unwrap();
    id
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn session_validity_expiry_revocation_and_rotation(pool: PgPool) {
    let (u, _) = user_with_org(&pool, "s").await;
    let now = OffsetDateTime::now_utc();
    let hour = time::Duration::hours(1);
    let id = new_session(&pool, u, b"tok-1", now + hour, now + hour).await;
    assert!(sessions::find_active(&pool, b"tok-1").await.unwrap().is_some());
    assert!(sessions::find_active(&pool, b"forged").await.unwrap().is_none());

    new_session(&pool, u, b"expired", now - hour, now + hour).await;
    assert!(sessions::find_active(&pool, b"expired").await.unwrap().is_none(), "absolute expiry");
    new_session(&pool, u, b"idle", now + hour, now - hour).await;
    assert!(sessions::find_active(&pool, b"idle").await.unwrap().is_none(), "idle expiry");

    // rotation: old token valid during grace, then not
    assert!(sessions::rotate(&pool, id, b"tok-1", b"tok-2", "csrf2", now + time::Duration::seconds(30)).await.unwrap());
    let prev = sessions::find_active(&pool, b"tok-1").await.unwrap().unwrap();
    assert!(prev.matched_previous);
    assert!(!sessions::find_active(&pool, b"tok-2").await.unwrap().unwrap().matched_previous);
    assert!(!sessions::rotate(&pool, id, b"tok-1", b"tok-3", "c", now).await.unwrap(), "stale rotation is a no-op");
    sqlx::query("UPDATE sessions SET previous_valid_until = now() - interval '1 second' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(sessions::find_active(&pool, b"tok-1").await.unwrap().is_none(), "grace elapsed");

    // revocation (and only by the owning user)
    assert!(matches!(sessions::revoke(&pool, Uuid::now_v7(), id, "x").await, Err(DbError::NotFound)));
    sessions::revoke(&pool, u, id, "user").await.unwrap();
    assert!(sessions::find_active(&pool, b"tok-2").await.unwrap().is_none());

    let a = new_session(&pool, u, b"a", now + hour, now + hour).await;
    new_session(&pool, u, b"b", now + hour, now + hour).await;
    assert_eq!(sessions::revoke_all(&pool, u, Some(a), "others").await.unwrap(), 1);
    assert!(sessions::find_active(&pool, b"a").await.unwrap().is_some());
    assert!(sessions::find_active(&pool, b"b").await.unwrap().is_none());
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn oidc_flow_is_single_use(pool: PgPool) {
    let f = sessions::OidcFlow {
        nonce: "n".into(),
        pkce_verifier: "v".into(),
        return_to: "/".into(),
        intent: "login".into(),
    };
    sessions::save_flow(&pool, b"state", &f, OffsetDateTime::now_utc() + time::Duration::minutes(10)).await.unwrap();
    assert!(sessions::take_flow(&pool, b"state").await.unwrap().is_some());
    assert!(sessions::take_flow(&pool, b"state").await.unwrap().is_none(), "replayed state");
    sessions::save_flow(&pool, b"old", &f, OffsetDateTime::now_utc() - time::Duration::minutes(1)).await.unwrap();
    assert!(sessions::take_flow(&pool, b"old").await.unwrap().is_none(), "expired state");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn invitation_accept_is_single_use(pool: PgPool) {
    let (owner, org) = user_with_org(&pool, "owner").await;
    let (guest, _) = user_with_org(&pool, "guest").await;
    let acc = access(org.id, owner);
    let exp = OffsetDateTime::now_utc() + time::Duration::days(1);
    let inv = orgs::create_invitation(
        &pool,
        &acc,
        "guest@example.com",
        rbac::builtin_role_id(OrgRole::Member),
        b"inv-token",
        owner,
        exp,
    )
    .await
    .unwrap();
    assert_eq!(orgs::find_invitation(&pool, b"inv-token").await.unwrap().role_key, "member");
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(orgs::accept_invitation(&mut tx, inv, guest).await.unwrap(), org.id);
    tx.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    assert!(matches!(orgs::accept_invitation(&mut tx, inv, guest).await, Err(DbError::NotFound)));
    let facts = orgs::facts_for_user_by_slug(&pool, guest, &org.slug).await.unwrap().facts;
    assert_eq!(facts.role.unwrap().builtin, Some(OrgRole::Member));
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn custom_roles_and_teams_cannot_cross_tenants(pool: PgPool) {
    let (ua, org_a) = user_with_org(&pool, "ra").await;
    let (ub, org_b) = user_with_org(&pool, "rb").await;
    orgs::sync_permissions(&pool).await.unwrap(); // done at API startup
    let mut tx = pool.begin().await.unwrap();
    let role_b = orgs::create_custom_role(
        &mut tx,
        &access(org_b.id, ub),
        "auditor",
        "Auditor",
        "",
        &PermissionSet::from_iter([app_authz::Permission::AuditRead]),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let a = access(org_a.id, ua);
    assert!(matches!(orgs::role_grant(&pool, &a, role_b).await, Err(DbError::NotFound)), "B's role invisible to A");
    // Even a direct write cannot attach B's custom role inside A (trigger).
    let r = sqlx::query("UPDATE organization_memberships SET role_id = $1 WHERE organization_id = $2 AND user_id = $3")
        .bind(role_b)
        .bind(org_a.id)
        .bind(ua)
        .execute(&pool)
        .await;
    assert!(r.is_err());
    // Team membership requires org membership (composite FK).
    let team = orgs::create_team(&pool, &a, "Core", "").await.unwrap();
    assert!(orgs::add_team_member(&pool, &a, team, ub).await.is_err(), "non-member cannot join A's team");
    orgs::add_team_member(&pool, &a, team, ua).await.unwrap();
    assert!(matches!(orgs::delete_team(&pool, &access(org_b.id, ub), team).await, Err(DbError::NotFound)));
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn slug_rules_enforced_before_insert(_pool: PgPool) {
    assert!(Slug::parse("admin").is_err());
}

fn job<'a>(key: Option<&'a str>, max: i32) -> jobs::NewJob<'a> {
    jobs::NewJob {
        queue: "default",
        kind: "test",
        payload: serde_json::json!({"n": 1}),
        priority: 0,
        max_attempts: max,
        run_at: None,
        idempotency_key: key,
        organization_id: None,
        trace_context: None,
    }
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn job_enqueue_is_idempotent(pool: PgPool) {
    let (a, created_a) = jobs::enqueue(&pool, &job(Some("k1"), 3)).await.unwrap();
    let (b, created_b) = jobs::enqueue(&pool, &job(Some("k1"), 3)).await.unwrap();
    assert!(created_a && !created_b);
    assert_eq!(a, b);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn concurrent_claims_never_share_a_job(pool: PgPool) {
    for _ in 0..20 {
        jobs::enqueue(&pool, &job(None, 3)).await.unwrap();
    }
    let (p1, p2) = (pool.clone(), pool.clone());
    let (a, b) = tokio::join!(jobs::claim(&p1, "default", "w1", 15, 30.0), jobs::claim(&p2, "default", "w2", 15, 30.0));
    let (a, b) = (a.unwrap(), b.unwrap());
    let mut ids: Vec<Uuid> = a.iter().chain(b.iter()).map(|j| j.id).collect();
    let n = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n, "no job claimed twice");
    assert_eq!(n, 20);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn failures_back_off_then_dead_letter(pool: PgPool) {
    let (id, _) = jobs::enqueue(&pool, &job(None, 2)).await.unwrap();
    let c = jobs::claim(&pool, "default", "w", 1, 30.0).await.unwrap();
    assert_eq!(c[0].attempts, 1);
    assert_eq!(jobs::fail(&pool, id, "w", "boom", false).await.unwrap(), "queued");
    assert!(jobs::claim(&pool, "default", "w", 1, 30.0).await.unwrap().is_empty(), "backoff delays retry");
    sqlx::query("UPDATE jobs SET run_at = now() WHERE id = $1").bind(id).execute(&pool).await.unwrap();
    jobs::claim(&pool, "default", "w", 1, 30.0).await.unwrap();
    assert_eq!(jobs::fail(&pool, id, "w", "boom again", false).await.unwrap(), "dead");
    // wrong worker cannot complete/fail someone else's job
    assert!(jobs::fail(&pool, id, "intruder", "x", false).await.is_err());
    jobs::retry_dead(&pool, id).await.unwrap();
    let c = jobs::claim(&pool, "default", "w", 1, 30.0).await.unwrap();
    assert_eq!(c[0].attempts, 1);
    assert!(jobs::complete(&pool, id, "w").await.unwrap());
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn expired_leases_are_reaped(pool: PgPool) {
    let (id, _) = jobs::enqueue(&pool, &job(None, 3)).await.unwrap();
    jobs::claim(&pool, "default", "crashed-worker", 1, 0.001).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(jobs::reap_expired(&pool).await.unwrap(), 1);
    let c = jobs::claim(&pool, "default", "w2", 1, 30.0).await.unwrap();
    assert_eq!((c[0].id, c[0].attempts), (id, 2));
    assert!(!jobs::complete(&pool, id, "crashed-worker").await.unwrap(), "stale worker cannot complete");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn account_deletion_scrubs_pii_and_releases_identity(pool: PgPool) {
    let (u, _) = user_with_org(&pool, "del").await;
    let mut tx = pool.begin().await.unwrap();
    users::mark_deleted(&mut tx, u).await.unwrap();
    tx.commit().await.unwrap();
    let row = users::get(&pool, u).await.unwrap();
    assert_eq!(row.status, "deleted");
    assert!(!row.email.contains("del@example.com"));
    let (fresh, created) = users::upsert_from_login(&pool, &claims("del", "del@example.com"), false).await.unwrap();
    assert!(created && fresh.id != u, "same IdP identity signs up as a new user");
}
