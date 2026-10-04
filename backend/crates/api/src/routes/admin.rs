//! System administration. Every handler calls `require_system` with its own permission:
//! an independent check per endpoint, never inferred from route prefixes or the frontend.

use std::time::Duration;

use app_authz::{Permission, SystemPermission as S, rbac};
use app_db::{audit::Outcome, jobs, orgs, sessions, users};
use app_domain::{OrgRole, SystemRole, UserStatus};
use app_errors::ApiError;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    audit,
    auth::{Principal, ReqMeta, extract::UserAuth, extract::require_system},
    errors::ResultExt,
    health::run_checks,
    routes::orgs::AuditQuery,
    state::AppState,
};

async fn guard(
    state: &AppState,
    u: &crate::auth::UserPrincipal,
    perm: S,
    meta: &ReqMeta,
) -> Result<Principal, ApiError> {
    let p = Principal::User(u.clone());
    require_system(state, &p, perm, meta).await?;
    Ok(p)
}

pub async fn overview(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
) -> Result<Json<Value>, ApiError> {
    guard(&state, &u, S::SystemRead, &meta).await?;
    let svc = state.svc()?;
    Ok(Json(json!({
        "counts": users::system_counts(&svc.db).await.api()?,
        "jobs": jobs::stats(&svc.db).await.api()?,
        "pool": app_db::pool_stats(&svc.db),
    })))
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    search: Option<String>,
    status: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

impl ListQuery {
    fn bounds(&self) -> (i64, i64) {
        let per = self.per_page.unwrap_or(25).clamp(1, 100);
        let page = self.page.unwrap_or(1).clamp(1, 10_000);
        (per, (page - 1) * per)
    }
}

pub async fn list_users(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    guard(&state, &u, S::UsersRead, &meta).await?;
    let (limit, offset) = q.bounds();
    let status = q.status.as_deref().filter(|s| UserStatus::parse(s).is_some());
    let (items, total) = users::admin_list(&state.svc()?.db, q.search.as_deref(), status, limit, offset).await.api()?;
    Ok(Json(json!({"items": items, "total": total, "page": q.page.unwrap_or(1), "per_page": limit})))
}

pub async fn get_user(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    guard(&state, &u, S::UsersRead, &meta).await?;
    let svc = state.svc()?;
    let row = users::get(&svc.db, id).await.api()?;
    let orgs = orgs::list_for_user(&svc.db, id).await.api()?;
    let sessions = sessions::list_active(&svc.db, id).await.api()?;
    Ok(Json(json!({
        "user": {"id": row.id, "email": row.email, "display_name": row.display_name, "status": row.status,
                 "system_role": row.system_role, "email_verified": row.email_verified,
                 "identity_provider": row.identity_provider, "created_at": row.created_at, "last_login_at": row.last_login_at},
        "organizations": orgs,
        "active_sessions": sessions.len(),
    })))
}

#[derive(Deserialize)]
pub struct UpdateUser {
    status: Option<String>,
    system_role: Option<String>,
}

pub async fn update_user(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
    Path(id): Path<Uuid>,
    Json(b): Json<UpdateUser>,
) -> Result<Json<Value>, ApiError> {
    let p = guard(&state, &u, S::UsersManage, &meta).await?;
    if id == u.user_id {
        return Err(ApiError::ForbiddenReason("cannot_modify_self"));
    }
    let svc = state.svc()?;
    let target = users::get(&svc.db, id).await.api()?;
    let mut tx = svc.db.begin().await.api()?;
    if let Some(s) = &b.status {
        let status = UserStatus::parse(s)
            .filter(|s| *s != UserStatus::Deleted)
            .ok_or(ApiError::validation("status", "active or suspended"))?;
        users::set_status(&mut *tx, id, status).await.api()?;
        if status == UserStatus::Suspended {
            sessions::revoke_all(&mut *tx, id, None, "suspended_by_admin").await.api()?;
        }
        let e =
            audit::event(Some(&p), &meta, state.config.auth.store_client_ip, "admin.user_updated", Outcome::Success)
                .target("user", id)
                .meta(json!({"status": status.as_str()}));
        app_db::audit::insert(&mut *tx, &e).await.api()?;
    }
    if let Some(r) = &b.system_role {
        if state.config.auth.system_roles_from_idp {
            return Err(ApiError::Conflict("system roles are managed by the identity provider".into()));
        }
        let role =
            SystemRole::parse(r).ok_or(ApiError::validation("system_role", "none, system_auditor or system_admin"))?;
        if target.system_role() == SystemRole::SystemAdmin
            && role != SystemRole::SystemAdmin
            && users::count_system_admins(&mut *tx).await.api()? <= 1
        {
            return Err(ApiError::ForbiddenReason("last_system_admin"));
        }
        users::set_system_role(&mut *tx, id, role).await.api()?;
        // Privilege change: force re-authentication everywhere.
        sessions::revoke_all(&mut *tx, id, None, "system_role_changed").await.api()?;
        let e =
            audit::event(Some(&p), &meta, state.config.auth.store_client_ip, "role.system_assigned", Outcome::Success)
                .target("user", id)
                .meta(json!({"from": target.system_role, "to": role.as_str()}));
        app_db::audit::insert(&mut *tx, &e).await.api()?;
    }
    tx.commit().await.api()?;
    Ok(Json(json!({"ok": true})))
}

pub async fn revoke_user_sessions(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let p = guard(&state, &u, S::UsersManage, &meta).await?;
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    let n = sessions::revoke_all(&mut *tx, id, None, "revoked_by_admin").await.api()?;
    let e =
        audit::event(Some(&p), &meta, state.config.auth.store_client_ip, "security.session_revoked", Outcome::Success)
            .target("user", id)
            .meta(json!({"count": n, "by": "system_admin"}));
    app_db::audit::insert(&mut *tx, &e).await.api()?;
    tx.commit().await.api()?;
    Ok(Json(json!({"revoked": n})))
}

pub async fn list_orgs(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    guard(&state, &u, S::OrgsRead, &meta).await?;
    let (limit, offset) = q.bounds();
    let (items, total) = orgs::admin_list(&state.svc()?.db, q.search.as_deref(), limit, offset).await.api()?;
    Ok(Json(json!({"items": items, "total": total, "page": q.page.unwrap_or(1), "per_page": limit})))
}

pub async fn roles_model(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
) -> Result<Json<Value>, ApiError> {
    guard(&state, &u, S::RolesRead, &meta).await?;
    let roles: Vec<Value> = OrgRole::ALL
        .iter()
        .rev()
        .map(|r| json!({"key": r.as_str(), "permissions": rbac::builtin_permissions(*r)}))
        .collect();
    let system: Vec<Value> = [SystemRole::SystemAdmin, SystemRole::SystemAuditor]
        .iter()
        .map(|r| json!({"key": r.as_str(), "permissions": rbac::system_permissions(*r).iter().map(|p| p.key()).collect::<Vec<_>>()}))
        .collect();
    Ok(Json(json!({
        "organization_roles": roles,
        "system_roles": system,
        "permissions": Permission::ALL.iter().map(|p| json!({"key": p.key(), "description": p.description()})).collect::<Vec<_>>(),
        "engine": state.config.authorization.engine,
        "system_roles_source": if state.config.auth.system_roles_from_idp { "identity_provider" } else { "application" },
    })))
}

pub async fn audit_log(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
    Query(q): Query<AuditQuery>,
) -> Result<Json<Value>, ApiError> {
    guard(&state, &u, S::AuditRead, &meta).await?;
    let (filter, cursor, limit) = q.filter()?;
    Ok(Json(json!(app_db::audit::list(&state.svc()?.db, None, &filter, cursor, limit).await.api()?)))
}

#[derive(Deserialize, Default)]
pub struct JobsQuery {
    status: Option<String>,
}

pub async fn list_jobs(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
    Query(q): Query<JobsQuery>,
) -> Result<Json<Value>, ApiError> {
    guard(&state, &u, S::JobsRead, &meta).await?;
    let svc = state.svc()?;
    let status = q.status.as_deref().filter(|s| matches!(*s, "queued" | "running" | "succeeded" | "dead"));
    Ok(Json(
        json!({"stats": jobs::stats(&svc.db).await.api()?, "items": jobs::list(&svc.db, status, 100).await.api()?}),
    ))
}

pub async fn retry_job(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let p = guard(&state, &u, S::JobsManage, &meta).await?;
    let svc = state.svc()?;
    jobs::retry_dead(&svc.db, id).await.api()?;
    let e = audit::event(Some(&p), &meta, state.config.auth.store_client_ip, "admin.job_retried", Outcome::Success)
        .target("job", id);
    audit::best_effort(&svc.db, &e).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn providers(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
) -> Result<Json<Value>, ApiError> {
    guard(&state, &u, S::ProvidersRead, &meta).await?;
    // Live health comes from the outbound engine when enabled; configuration is shown otherwise.
    let items: Vec<Value> = state
        .config
        .providers
        .definitions
        .iter()
        .map(|(name, d)| {
            json!({"name": name, "base_url": d.base_url, "max_concurrency": d.max_concurrency,
                   "requests_per_second": d.requests_per_second, "tokens_per_minute": d.tokens_per_minute})
        })
        .collect();
    Ok(Json(json!({"items": items})))
}

pub async fn system(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
) -> Result<Json<Value>, ApiError> {
    guard(&state, &u, S::SystemRead, &meta).await?;
    let checks = run_checks(&state.health, Duration::from_secs(2)).await;
    let c = &state.config;
    Ok(Json(json!({
        "build": state.build,
        "environment": c.environment,
        "checks": checks,
        "pool": state.services.as_ref().map(|s| app_db::pool_stats(&s.db)),
        "modules": {
            "cache": c.cache.backend, "rate_limit": c.rate_limit.enabled, "messaging": c.messaging.enabled,
            "analytics": c.analytics.enabled, "organizations": c.tenancy.organizations,
            "authorization_engine": c.authorization.engine,
        },
        "identity": {"provider": c.auth.provider, "issuer": c.auth.issuer_url, "profile": c.auth.profile,
                     "require_mfa_for_system_admin": c.auth.require_mfa_for_system_admin},
    })))
}
