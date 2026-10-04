//! Browser authentication endpoints (BFF).
//!
//! ```text
//! GET  /auth/login     ?return_to&method&login_hint  → 302 to the IdP (code + PKCE + nonce)
//! GET  /auth/register  ?return_to&login_hint         → 302 to the IdP with prompt=create
//! GET  /auth/reauth    ?return_to                    → 302 to the IdP with prompt=login, max_age=0
//! GET  /auth/callback  ?code&state                   → verify, create session, 302 to return_to
//! POST /auth/logout                                  → revoke session; JSON {redirect}
//! GET  /api/v1/session                               → who am I + CSRF token + login config
//! ```

use app_auth::{
    oidc::{Intent, LoginOptions, safe_return_to},
    tokens,
};
use app_db::{audit::Outcome, orgs, sessions, users};
use app_domain::{SystemRole, UserStatus};
use app_errors::ApiError;
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use serde_json::json;
use time::OffsetDateTime;

use super::{
    Principal, ReqMeta, cookies,
    extract::{MaybeAuth, UserAuth},
};
use crate::{audit, errors::ResultExt, state::AppState};

#[derive(Deserialize, Default)]
pub struct BeginQuery {
    return_to: Option<String>,
    method: Option<String>,
    login_hint: Option<String>,
}

fn login_error(code: &str) -> Response {
    Redirect::to(&format!("/login?error={code}")).into_response()
}

async fn begin(state: &AppState, intent: Intent, q: BeginQuery) -> Response {
    let Ok(svc) = state.svc() else { return login_error("unavailable") };
    let cfg = &state.config.auth;
    if intent == Intent::Register && !cfg.allow_registration {
        return login_error("registration_closed");
    }
    let social = match q.method.as_deref() {
        None | Some("passkey" | "password" | "email" | "sso") => None,
        Some(name) if cfg.methods.social.contains_key(name) => Some(name.to_string()),
        Some(_) => return login_error("unknown_method"),
    };
    let opts = LoginOptions { login_hint: q.login_hint.filter(|h| h.contains('@')), social };
    let pending = match svc.oidc.begin(intent, &opts).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "cannot start login");
            return login_error("idp_unavailable");
        }
    };
    let flow = sessions::OidcFlow {
        nonce: pending.nonce,
        pkce_verifier: pending.pkce_verifier,
        return_to: safe_return_to(q.return_to.as_deref()),
        intent: intent.as_str().into(),
    };
    let expires = OffsetDateTime::now_utc() + time::Duration::minutes(10);
    if let Err(e) = sessions::save_flow(&svc.db, &tokens::token_hash(&pending.state), &flow, expires).await {
        return crate::errors::db(e).into_response();
    }
    let mut res = Redirect::to(&pending.authorize_url).into_response();
    if let Ok(c) = cookies::flow_cookie(cfg, &pending.state) {
        res.headers_mut().append(header::SET_COOKIE, c);
    }
    res.headers_mut().insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    res
}

pub async fn login(State(state): State<AppState>, Query(q): Query<BeginQuery>) -> Response {
    begin(&state, Intent::Login, q).await
}

pub async fn register(State(state): State<AppState>, Query(q): Query<BeginQuery>) -> Response {
    begin(&state, Intent::Register, q).await
}

pub async fn reauth(State(state): State<AppState>, Query(q): Query<BeginQuery>) -> Response {
    begin(&state, Intent::Reauth, BeginQuery { method: None, ..q }).await
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Map IdP project roles to the system trust level (highest wins).
fn system_role_from_idp(roles: &[String]) -> SystemRole {
    if roles.iter().any(|r| r == "system_admin") {
        SystemRole::SystemAdmin
    } else if roles.iter().any(|r| r == "system_auditor") {
        SystemRole::SystemAuditor
    } else {
        SystemRole::None
    }
}

pub async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    meta: ReqMeta,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let cfg = &state.config.auth;
    let Ok(svc) = state.svc() else { return login_error("unavailable") };
    let store_ip = cfg.store_client_ip;
    let clear_flow = cookies::clear_flow_cookie(cfg).ok();
    let fail = |reason: &'static str| {
        let svc = svc.clone();
        let meta = meta.clone();
        let clear_flow = clear_flow.clone();
        async move {
            let e = audit::event(None, &meta, store_ip, "user.login", Outcome::Failure).meta(json!({"reason": reason}));
            audit::best_effort(&svc.db, &e).await;
            let mut r = login_error(reason);
            if let Some(c) = clear_flow {
                r.headers_mut().append(header::SET_COOKIE, c);
            }
            r
        }
    };
    if q.error.is_some() {
        return fail("idp_error").await;
    }
    let (Some(code), Some(st)) = (q.code, q.state) else { return fail("invalid_callback").await };
    // Login CSRF: the state must match the cookie set on this browser when the flow began.
    let cookie_state = cookies::get(&headers, &cookies::flow_cookie_name(cfg)).unwrap_or_default();
    if !tokens::ct_eq(&cookie_state, &st) {
        return fail("state_mismatch").await;
    }
    let flow = match sessions::take_flow(&svc.db, &tokens::token_hash(&st)).await {
        Ok(Some(f)) => f,
        Ok(None) => return fail("flow_expired").await,
        Err(e) => return crate::errors::db(e).into_response(),
    };
    let login = match svc.oidc.complete(&code, &flow.nonce, &flow.pkce_verifier).await {
        Ok(l) => l,
        Err(app_auth::oidc::OidcError::Unavailable(_)) => return fail("idp_unavailable").await,
        Err(e) => {
            tracing::warn!(error = %e, "login rejected");
            return fail("login_rejected").await;
        }
    };
    if login.email.is_empty() {
        return fail("email_required").await;
    }

    let mfa = app_auth::amr_is_mfa(&login.amr);
    let bootstrap =
        login.email_verified && cfg.bootstrap_system_admins.iter().any(|e| e.eq_ignore_ascii_case(&login.email));
    let previous = cookies::get(&headers, &cfg.effective_cookie_name()).filter(|t| tokens::plausible_token(t));
    let token = tokens::new_token();
    let session_id = app_domain::new_id();
    let now = OffsetDateTime::now_utc();
    let expires = now + time::Duration::minutes(cfg.session_absolute_ttl_minutes as i64);

    let result: Result<(), ApiError> = async {
        let mut tx = svc.db.begin().await.api()?;
        let claims = users::IdentityClaims {
            issuer: &login.issuer,
            subject: &login.subject,
            email: &login.email,
            email_verified: login.email_verified,
            name: login.name.as_deref(),
            picture: login.picture.as_deref(),
        };
        let (user, created) = users::upsert_from_login(&mut *tx, &claims, bootstrap).await.api()?;
        if user.status() != UserStatus::Active {
            return Err(ApiError::ForbiddenReason("account_inactive"));
        }
        let actor = Principal::User(super::UserPrincipal {
            user_id: user.id,
            session_id,
            email: user.email.clone(),
            email_verified: user.email_verified,
            display_name: user.display_name.clone(),
            system_role: user.system_role(),
            mfa,
            amr: login.amr.clone(),
            auth_time: login.auth_time,
            csrf_token: String::new(),
            external_subject: user.external_subject.clone(),
        });
        if created {
            app_db::audit::insert(&mut *tx, &audit::event(Some(&actor), &meta, store_ip, "user.created", Outcome::Success)).await.api()?;
        }
        if cfg.system_roles_from_idp {
            let desired = system_role_from_idp(&login.idp_roles);
            if desired != user.system_role() {
                users::set_system_role(&mut *tx, user.id, desired).await.api()?;
                let e = audit::event(Some(&actor), &meta, store_ip, "role.system_synced", Outcome::Success)
                    .target("user", user.id)
                    .meta(json!({"from": user.system_role().as_str(), "to": desired.as_str(), "source": "identity_provider"}));
                app_db::audit::insert(&mut *tx, &e).await.api()?;
            }
        }
        orgs::ensure_personal_org(&mut tx, user.id, &user.display_name).await.api()?;
        // A login always creates a brand-new session id (no fixation) and ends the old one.
        if let Some(old) = &previous
            && let Some(s) = sessions::find_active(&mut *tx, &tokens::token_hash(old)).await.api()?
        {
            sessions::revoke(&mut *tx, s.user_id, s.id, "replaced_by_login").await.api()?;
        }
        let id_token_enc = svc.cipher.encrypt(login.id_token.as_bytes(), session_id.as_bytes());
        let ip = if cfg.store_client_ip { meta.ip.map(ipnet::IpNet::from) } else { None };
        let ua = if cfg.store_user_agent { meta.user_agent.as_deref() } else { None };
        let csrf = tokens::new_token();
        sessions::create(
            &mut *tx,
            &sessions::NewSession {
                id: session_id,
                user_id: user.id,
                token_hash: &tokens::token_hash(&token),
                csrf_token: &csrf,
                expires_at: expires,
                idle_expires_at: now + time::Duration::minutes(cfg.session_idle_ttl_minutes as i64),
                auth_time: login.auth_time,
                amr: &login.amr,
                mfa,
                ip,
                user_agent: ua,
                id_token_enc: Some(&id_token_enc),
            },
        )
        .await
        .api()?;
        let e = audit::event(Some(&actor), &meta, store_ip, "user.login", Outcome::Success)
            .meta(json!({"amr": login.amr, "mfa": mfa, "intent": flow.intent, "session_id": session_id}));
        app_db::audit::insert(&mut *tx, &e).await.api()?;
        tx.commit().await.api()?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            let mut res = Redirect::to(&flow.return_to).into_response();
            if let Ok(c) = cookies::session_cookie(cfg, &token, expires) {
                res.headers_mut().append(header::SET_COOKIE, c);
            }
            if let Ok(c) = cookies::clear_flow_cookie(cfg) {
                res.headers_mut().append(header::SET_COOKIE, c);
            }
            res
        }
        Err(ApiError::ForbiddenReason("account_inactive")) => fail("account_inactive").await,
        Err(e) => e.into_response(),
    }
}

/// End this session. Returns where the browser should go next (the IdP end-session endpoint
/// when available, so the IdP session ends too).
pub async fn logout(State(state): State<AppState>, UserAuth(u): UserAuth, meta: ReqMeta) -> Result<Response, ApiError> {
    let svc = state.svc()?;
    let id_token = sessions::id_token_enc(&svc.db, u.session_id)
        .await
        .api()?
        .and_then(|enc| svc.cipher.decrypt(&enc, u.session_id.as_bytes()).ok())
        .and_then(|b| String::from_utf8(b).ok());
    let mut tx = svc.db.begin().await.api()?;
    sessions::revoke(&mut *tx, u.user_id, u.session_id, "logout").await.api()?;
    let p = Principal::User(u.clone());
    app_db::audit::insert(
        &mut *tx,
        &audit::event(Some(&p), &meta, state.config.auth.store_client_ip, "user.logout", Outcome::Success),
    )
    .await
    .api()?;
    tx.commit().await.api()?;
    let redirect = svc.oidc.logout_url(id_token.as_deref()).await.unwrap_or_else(|| "/login".into());
    let mut res = Json(crate::dto::LogoutResponse { redirect }).into_response();
    if let Ok(c) = cookies::clear_session_cookie(&state.config.auth) {
        res.headers_mut().append(header::SET_COOKIE, c);
    }
    Ok(res)
}

pub fn login_config(state: &AppState) -> crate::dto::LoginConfig {
    let a = &state.config.auth;
    crate::dto::LoginConfig {
        provider: crate::dto::enum_str(&a.provider),
        passkey: a.methods.passkey,
        password: a.methods.password,
        social: a.methods.social.keys().cloned().collect(),
        enterprise_sso: a.methods.enterprise_sso,
        registration: a.allow_registration,
    }
}

/// Current session for the SPA. Always 200 so the login page can render its configuration.
pub async fn session(State(state): State<AppState>, MaybeAuth(p): MaybeAuth) -> Result<Response, ApiError> {
    use crate::dto::{Features, SessionMeta, SessionResponse, SessionUser};
    let features = Features {
        organizations: state.config.tenancy.organizations,
        org_creation: state.config.tenancy.allow_org_creation,
        admin: state.config.admin.enabled,
        auth_profile: crate::dto::enum_str(&state.config.auth.profile),
    };
    let login = login_config(&state);
    let Some(Principal::User(u)) = p else {
        return Ok(Json(SessionResponse {
            authenticated: false,
            csrf_token: None,
            user: None,
            session: None,
            organizations: Vec::new(),
            unread_notifications: 0,
            features,
            login,
        })
        .into_response());
    };
    let svc = state.svc()?;
    let organizations = orgs::list_for_user(&svc.db, u.user_id).await.api()?;
    let unread = app_db::notifications::unread_count(&svc.db, u.user_id).await.api()?;
    let mut res = Json(SessionResponse {
        authenticated: true,
        csrf_token: Some(u.csrf_token.clone()),
        user: Some(SessionUser {
            id: u.user_id,
            email: u.email.clone(),
            email_verified: u.email_verified,
            display_name: u.display_name.clone(),
            system_role: u.system_role,
        }),
        session: Some(SessionMeta {
            id: u.session_id,
            mfa: u.mfa,
            amr: u.amr.clone(),
            auth_time: u.auth_time.unix_timestamp(),
        }),
        organizations,
        unread_notifications: unread,
        features,
        login,
    })
    .into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    Ok(res)
}

pub async fn not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}
