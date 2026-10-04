//! Account center (the signed-in human only) and the personal dashboard.
//! Credentials (passkeys, MFA, passwords, linked identities) belong to the identity provider;
//! these endpoints show their state and delegate changes through `IdentityAdmin`.

use app_auth::idp_admin::IdpAdminError;
use app_db::{audit::Outcome, notifications, orgs, sessions, users};
use app_domain::user::validate_display_name;
use app_errors::ApiError;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    audit,
    auth::{Principal, ReqMeta, UserPrincipal, cookies, extract::UserAuth},
    errors::ResultExt,
    state::AppState,
};

fn recent_auth(state: &AppState, u: &UserPrincipal) -> Result<(), ApiError> {
    let window = time::Duration::minutes(state.config.auth.reauth_window_minutes as i64);
    if OffsetDateTime::now_utc() - u.auth_time > window {
        return Err(ApiError::ForbiddenReason("reauth_required"));
    }
    Ok(())
}

pub async fn get_profile(State(state): State<AppState>, UserAuth(u): UserAuth) -> Result<Json<Value>, ApiError> {
    let row = users::get(&state.svc()?.db, u.user_id).await.api()?;
    Ok(Json(json!({
        "id": row.id, "email": row.email, "email_verified": row.email_verified,
        "display_name": row.display_name, "avatar_url": row.avatar_url, "preferences": row.preferences,
        "system_role": row.system_role, "created_at": row.created_at, "last_login_at": row.last_login_at,
        "identity": {"provider": row.identity_provider, "subject": row.external_subject},
    })))
}

#[derive(Deserialize)]
pub struct ProfileUpdate {
    display_name: String,
    avatar_url: Option<String>,
}

pub async fn update_profile(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
    Json(body): Json<ProfileUpdate>,
) -> Result<Json<Value>, ApiError> {
    let name = validate_display_name(&body.display_name).map_err(|e| ApiError::validation(e.field, e.message))?;
    let avatar = body.avatar_url.filter(|a| !a.is_empty());
    if let Some(a) = &avatar
        && (!a.starts_with("https://") || a.len() > 512)
    {
        return Err(ApiError::validation("avatar_url", "must be an https URL"));
    }
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    let row = users::update_profile(&mut *tx, u.user_id, &name, avatar.as_deref()).await.api()?;
    let p = Principal::User(u);
    let e = audit::event(Some(&p), &meta, state.config.auth.store_client_ip, "user.profile_updated", Outcome::Success)
        .target("user", row.id)
        .meta(json!({"fields": ["display_name", "avatar_url"]}));
    app_db::audit::insert(&mut *tx, &e).await.api()?;
    tx.commit().await.api()?;
    Ok(Json(json!({"display_name": row.display_name, "avatar_url": row.avatar_url})))
}

pub async fn update_preferences(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    Json(prefs): Json<Value>,
) -> Result<StatusCode, ApiError> {
    if !prefs.is_object() || prefs.to_string().len() > 8 * 1024 {
        return Err(ApiError::validation("preferences", "must be a JSON object under 8 KiB"));
    }
    users::update_preferences(&state.svc()?.db, u.user_id, &prefs).await.api()?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn security(State(state): State<AppState>, UserAuth(u): UserAuth) -> Result<Json<Value>, ApiError> {
    let svc = state.svc()?;
    let overview = match svc.idp_admin.security_overview(&u.external_subject).await {
        Ok(o) => json!(o),
        Err(e) => {
            tracing::warn!(error = %e, "identity provider security overview unavailable");
            json!({"methods": [], "mfa_enabled": null, "passkeys": null, "manage_url": null, "unavailable": true})
        }
    };
    let filter = app_db::audit::AuditFilter { actor_id: Some(u.user_id), ..Default::default() };
    let events = app_db::audit::list(&svc.db, None, &filter, None, 20).await.api()?;
    let security_events: Vec<_> = events
        .items
        .into_iter()
        .filter(|e| {
            e.action.starts_with("security.") || e.action.starts_with("user.login") || e.action == "user.logout"
        })
        .collect();
    Ok(Json(json!({
        "session": {"mfa": u.mfa, "amr": u.amr, "auth_time": u.auth_time.unix_timestamp()},
        "identity_provider": overview,
        "events": security_events,
        "reauth_window_minutes": state.config.auth.reauth_window_minutes,
    })))
}

pub async fn remove_method(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
    Path((kind, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    recent_auth(&state, &u)?;
    let svc = state.svc()?;
    match svc.idp_admin.remove_method(&u.external_subject, &kind, &id).await {
        Ok(()) => {}
        Err(IdpAdminError::Unsupported) => return Err(ApiError::ForbiddenReason("managed_by_identity_provider")),
        Err(IdpAdminError::Api(e)) => return Err(ApiError::Upstream(e)),
    }
    let p = Principal::User(u);
    let action = if kind == "passkey" { "security.passkey_removed" } else { "security.mfa_removed" };
    let e = audit::event(Some(&p), &meta, state.config.auth.store_client_ip, action, Outcome::Success)
        .meta(json!({"kind": kind}));
    audit::best_effort(&svc.db, &e).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn resend_verification(State(state): State<AppState>, UserAuth(u): UserAuth) -> Result<StatusCode, ApiError> {
    if u.email_verified {
        return Err(ApiError::Conflict("email already verified".into()));
    }
    match state.svc()?.idp_admin.resend_email_verification(&u.external_subject).await {
        Ok(()) => Ok(StatusCode::ACCEPTED),
        Err(IdpAdminError::Unsupported) => Err(ApiError::ForbiddenReason("managed_by_identity_provider")),
        Err(IdpAdminError::Api(e)) => Err(ApiError::Upstream(e)),
    }
}

pub async fn list_sessions(State(state): State<AppState>, UserAuth(u): UserAuth) -> Result<Json<Value>, ApiError> {
    let list = sessions::list_active(&state.svc()?.db, u.user_id).await.api()?;
    let items: Vec<Value> = list
        .into_iter()
        .map(|s| {
            let current = s.id == u.session_id;
            let mut v = json!(s);
            v["current"] = json!(current);
            v
        })
        .collect();
    Ok(Json(json!({"items": items})))
}

pub async fn revoke_session(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    sessions::revoke(&mut *tx, u.user_id, id, "user_revoked").await.api()?;
    let current = id == u.session_id;
    let p = Principal::User(u);
    let e =
        audit::event(Some(&p), &meta, state.config.auth.store_client_ip, "security.session_revoked", Outcome::Success)
            .target("session", id);
    app_db::audit::insert(&mut *tx, &e).await.api()?;
    tx.commit().await.api()?;
    let mut res = StatusCode::NO_CONTENT.into_response();
    if current && let Ok(c) = cookies::clear_session_cookie(&state.config.auth) {
        res.headers_mut().append(header::SET_COOKIE, c);
    }
    Ok(res)
}

#[derive(Deserialize, Default)]
pub struct RevokeAll {
    #[serde(default)]
    include_current: bool,
}

pub async fn revoke_all_sessions(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
    body: Option<Json<RevokeAll>>,
) -> Result<Response, ApiError> {
    let include_current = body.map(|b| b.0.include_current).unwrap_or(false);
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    let except = (!include_current).then_some(u.session_id);
    let n = sessions::revoke_all(&mut *tx, u.user_id, except, "user_revoked_all").await.api()?;
    let p = Principal::User(u);
    let e =
        audit::event(Some(&p), &meta, state.config.auth.store_client_ip, "security.sessions_revoked", Outcome::Success)
            .meta(json!({"count": n, "include_current": include_current}));
    app_db::audit::insert(&mut *tx, &e).await.api()?;
    tx.commit().await.api()?;
    let mut res = Json(json!({"revoked": n})).into_response();
    if include_current && let Ok(c) = cookies::clear_session_cookie(&state.config.auth) {
        res.headers_mut().append(header::SET_COOKIE, c);
    }
    Ok(res)
}

pub async fn activity(State(state): State<AppState>, UserAuth(u): UserAuth) -> Result<Json<Value>, ApiError> {
    let filter = app_db::audit::AuditFilter { actor_id: Some(u.user_id), ..Default::default() };
    let page = app_db::audit::list(&state.svc()?.db, None, &filter, None, 50).await.api()?;
    Ok(Json(json!(page)))
}

#[derive(Deserialize)]
pub struct DeleteAccount {
    confirm_email: String,
}

/// Account deletion workflow:
/// 1. typed confirmation of the account email;
/// 2. recent authentication (step-up via /auth/reauth);
/// 3. policy hook: refuse while the user is the sole owner of a shared organisation;
/// 4. deactivate the identity at the IdP when supported (best effort; logged);
/// 5. soft-delete + PII scrub + revoke all sessions + audit, in one transaction.
pub async fn delete_account(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
    Json(body): Json<DeleteAccount>,
) -> Result<Response, ApiError> {
    if !body.confirm_email.trim().eq_ignore_ascii_case(&u.email) {
        return Err(ApiError::validation("confirm_email", "does not match your account email"));
    }
    recent_auth(&state, &u)?;
    let svc = state.svc()?;
    let blocking = orgs::sole_owner_orgs(&svc.db, u.user_id).await.api()?;
    if !blocking.is_empty() {
        return Err(ApiError::Conflict(format!(
            "transfer ownership or delete these organizations first: {}",
            blocking.join(", ")
        )));
    }
    match svc.idp_admin.deactivate_user(&u.external_subject).await {
        Ok(()) | Err(IdpAdminError::Unsupported) => {}
        Err(IdpAdminError::Api(e)) => {
            tracing::warn!(error = %e, "IdP deactivation failed; continuing with local deletion")
        }
    }
    let mut tx = svc.db.begin().await.api()?;
    users::mark_deleted(&mut tx, u.user_id).await.api()?;
    sessions::revoke_all(&mut *tx, u.user_id, None, "account_deleted").await.api()?;
    let p = Principal::User(u.clone());
    let e = audit::event(Some(&p), &meta, state.config.auth.store_client_ip, "user.deleted", Outcome::Success)
        .target("user", u.user_id);
    app_db::audit::insert(&mut *tx, &e).await.api()?;
    tx.commit().await.api()?;
    let mut res = StatusCode::NO_CONTENT.into_response();
    if let Ok(c) = cookies::clear_session_cookie(&state.config.auth) {
        res.headers_mut().append(header::SET_COOKIE, c);
    }
    Ok(res)
}

#[derive(Deserialize, Default)]
pub struct NotifQuery {
    #[serde(default)]
    unread: bool,
}

pub async fn list_notifications(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    Query(q): Query<NotifQuery>,
) -> Result<Json<Value>, ApiError> {
    let svc = state.svc()?;
    let items = notifications::list(&svc.db, u.user_id, q.unread, 50).await.api()?;
    let unread = notifications::unread_count(&svc.db, u.user_id).await.api()?;
    Ok(Json(json!({"items": items, "unread": unread})))
}

pub async fn read_notification(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    notifications::mark_read(&state.svc()?.db, u.user_id, id).await.api()?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn read_all_notifications(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
) -> Result<Json<Value>, ApiError> {
    let n = notifications::mark_all_read(&state.svc()?.db, u.user_id).await.api()?;
    Ok(Json(json!({"marked": n})))
}

/// Personal dashboard: modular widgets the frontend renders independently.
pub async fn dashboard(State(state): State<AppState>, UserAuth(u): UserAuth) -> Result<Json<Value>, ApiError> {
    let svc = state.svc()?;
    let my_orgs = orgs::list_for_user(&svc.db, u.user_id).await.api()?;
    let unread = notifications::unread_count(&svc.db, u.user_id).await.api()?;
    let recent = notifications::list(&svc.db, u.user_id, false, 5).await.api()?;
    let filter = app_db::audit::AuditFilter { actor_id: Some(u.user_id), ..Default::default() };
    let activity = app_db::audit::list(&svc.db, None, &filter, None, 10).await.api()?;
    let sessions = sessions::list_active(&svc.db, u.user_id).await.api()?;
    Ok(Json(json!({
        "widgets": {
            "account": {"display_name": u.display_name, "email": u.email, "email_verified": u.email_verified,
                        "organizations": my_orgs.len(), "system_role": u.system_role},
            "security": {"mfa_this_session": u.mfa, "active_sessions": sessions.len(), "email_verified": u.email_verified},
            "notifications": {"unread": unread, "recent": recent},
            "activity": activity.items,
            "organizations": my_orgs,
        }
    })))
}
