//! Request authentication (who is calling) for API and auth routes.
//!
//! Exactly one principal per request, resolved by [`authenticate`]:
//! 1. `Authorization: Bearer <api key>` → API key (organisation credential)
//! 2. `Authorization: Bearer <JWT>` → service account (machine-to-machine)
//! 3. session cookie → user session (browser, BFF)
//!
//! A present-but-invalid bearer credential is a hard 401: it never falls back to the cookie.
//! Cookie-authenticated unsafe requests must carry `X-CSRF-Token` equal to the session's token
//! (defence in depth on top of the Fetch-Metadata CSRF layer). Bearer requests are exempt
//! because browsers never attach them automatically.

pub mod cookies;
pub mod extract;
pub mod routes;

use std::time::Duration as StdDuration;

use app_auth::{api_keys, tokens};
use app_authz::{Actor, PermissionSet, UserActor};
use app_db::sessions;
use app_domain::{SystemRole, UserStatus};
use app_errors::ApiError;
use axum::{
    extract::{Request, State},
    http::{HeaderValue, Method, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{middleware::ClientIp, state::AppState};

/// A browser session's user.
#[derive(Debug, Clone)]
pub struct UserPrincipal {
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub email: String,
    pub email_verified: bool,
    pub display_name: String,
    pub system_role: SystemRole,
    pub mfa: bool,
    pub amr: Vec<String>,
    pub auth_time: OffsetDateTime,
    pub csrf_token: String,
    pub external_subject: String,
}

#[derive(Debug, Clone)]
pub enum Principal {
    User(UserPrincipal),
    Service { client_id: Uuid, organization_id: Uuid, scopes: PermissionSet, name: String },
    ApiKey { key_id: Uuid, organization_id: Uuid, scopes: PermissionSet, created_by: Option<Uuid>, name: String },
}

impl Principal {
    pub fn actor(&self) -> Actor {
        match self {
            Principal::User(u) => Actor::User(UserActor {
                id: u.user_id,
                system_role: u.system_role,
                active: true, // inactive users never get a principal
                mfa: u.mfa,
                auth_age: (OffsetDateTime::now_utc() - u.auth_time).try_into().unwrap_or(StdDuration::MAX),
            }),
            Principal::Service { client_id, organization_id, scopes, .. } => {
                Actor::Service { id: *client_id, organization_id: *organization_id, scopes: *scopes }
            }
            Principal::ApiKey { key_id, organization_id, scopes, created_by, .. } => Actor::ApiKey {
                id: *key_id,
                organization_id: *organization_id,
                scopes: *scopes,
                created_by: *created_by,
            },
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Principal::User(_) => "user",
            Principal::Service { .. } => "service",
            Principal::ApiKey { .. } => "api_key",
        }
    }

    pub fn id(&self) -> Uuid {
        match self {
            Principal::User(u) => u.user_id,
            Principal::Service { client_id, .. } => *client_id,
            Principal::ApiKey { key_id, .. } => *key_id,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Principal::User(u) => u.email.clone(),
            Principal::Service { name, .. } | Principal::ApiKey { name, .. } => name.clone(),
        }
    }
}

fn unauthorized(code: &'static str) -> Response {
    let mut r = ApiError::Unauthenticated.into_response();
    if let Ok(v) = HeaderValue::from_str(&format!("Bearer error=\"{code}\"")) {
        r.headers_mut().insert(header::WWW_AUTHENTICATE, v);
    }
    r
}

fn is_unsafe(m: &Method) -> bool {
    !matches!(*m, Method::GET | Method::HEAD | Method::OPTIONS)
}

pub async fn authenticate(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let Some(svc) = state.services.clone() else { return next.run(req).await };
    let cfg = &state.config.auth;
    let mut set_cookie: Option<HeaderValue> = None;

    let bearer = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let principal = if let Some(h) = bearer {
        let Some(token) = h.strip_prefix("Bearer ").map(str::trim) else { return unauthorized("invalid_request") };
        if let Some(parsed) = api_keys::parse(&cfg.api_key_prefix, token) {
            match resolve_api_key(&svc, &parsed).await {
                Ok(Some(p)) => Some(p),
                Ok(None) => return unauthorized("invalid_token"),
                Err(e) => return e.into_response(),
            }
        } else if token.matches('.').count() == 2 {
            match svc.service_tokens.verify(token).await {
                Ok(claims) => match app_db::api_keys::find_service_client(&svc.db, &claims.sub).await {
                    Ok(Some(client)) => {
                        // Effective scopes: registered scopes ∩ scopes in this token.
                        let registered =
                            PermissionSet::parse_keys(client.scopes.iter().map(String::as_str)).unwrap_or_default();
                        let granted = PermissionSet::parse_keys(claims.scopes()).unwrap_or_default();
                        Some(Principal::Service {
                            client_id: client.id,
                            organization_id: client.organization_id,
                            scopes: registered.intersect(&granted),
                            name: client.name,
                        })
                    }
                    Ok(None) => return unauthorized("invalid_token"),
                    Err(e) => return crate::errors::db(e).into_response(),
                },
                Err(app_auth::service::ServiceTokenError::Unavailable(_)) => {
                    return ApiError::Unavailable("identity provider").into_response();
                }
                Err(e) => {
                    tracing::debug!(error = %e, "service token rejected");
                    return unauthorized("invalid_token");
                }
            }
        } else {
            return unauthorized("invalid_token");
        }
    } else if let Some(token) = cookies::get(req.headers(), &cfg.effective_cookie_name()) {
        let found = if tokens::plausible_token(&token) {
            match sessions::find_active_with_user(&svc.db, &tokens::token_hash(&token)).await {
                Ok(s) => s,
                Err(e) => return crate::errors::db(e).into_response(),
            }
        } else {
            None
        };
        match found {
            Some(s) if UserStatus::parse(&s.user_status) == Some(UserStatus::Active) => {
                if is_unsafe(req.method()) {
                    let sent = req.headers().get("x-csrf-token").and_then(|v| v.to_str().ok()).unwrap_or("");
                    if !tokens::ct_eq(sent, &s.csrf_token) {
                        return ApiError::ForbiddenReason("csrf_failed").into_response();
                    }
                }
                let now = OffsetDateTime::now_utc();
                if now - s.last_seen_at > time::Duration::seconds(60) {
                    let idle = now + time::Duration::minutes(cfg.session_idle_ttl_minutes as i64);
                    if let Err(e) = sessions::touch(&svc.db, s.session_id, idle).await {
                        tracing::warn!(error = %e, "session touch failed");
                    }
                }
                // Periodic rotation limits the value of a stolen cookie. Only on safe methods
                // (no racing with state changes) and never from a previous-token match.
                let rotate_due = now - s.rotated_at > time::Duration::minutes(cfg.session_rotate_minutes as i64);
                if rotate_due && !is_unsafe(req.method()) && !s.matched_previous {
                    let fresh = tokens::new_token();
                    match sessions::rotate(
                        &svc.db,
                        s.session_id,
                        &tokens::token_hash(&token),
                        &tokens::token_hash(&fresh),
                        &s.csrf_token,
                        now + time::Duration::seconds(60),
                    )
                    .await
                    {
                        Ok(true) => set_cookie = cookies::session_cookie(cfg, &fresh, s.expires_at).ok(),
                        Ok(false) => {}
                        Err(e) => tracing::warn!(error = %e, "session rotation failed"),
                    }
                }
                Some(Principal::User(UserPrincipal {
                    user_id: s.user_id,
                    session_id: s.session_id,
                    email: s.email,
                    email_verified: s.email_verified,
                    display_name: s.display_name,
                    system_role: SystemRole::parse(&s.system_role).unwrap_or(SystemRole::None),
                    mfa: s.mfa,
                    amr: s.amr,
                    auth_time: s.auth_time,
                    csrf_token: s.csrf_token,
                    external_subject: s.external_subject,
                }))
            }
            Some(s) => {
                // Suspended or deleted: end every session of this user now.
                let _ = sessions::revoke_all(&svc.db, s.user_id, None, "account_inactive").await;
                set_cookie = cookies::clear_session_cookie(cfg).ok();
                None
            }
            None => {
                set_cookie = cookies::clear_session_cookie(cfg).ok();
                None
            }
        }
    } else {
        None
    };

    if let Some(p) = &principal {
        req.extensions_mut().insert(crate::middleware::RateLimitKey(format!("{}:{}", p.kind(), p.id())));
        req.extensions_mut().insert(p.clone());
    }
    let mut res = next.run(req).await;
    if let Some(c) = set_cookie {
        res.headers_mut().append(header::SET_COOKIE, c);
    }
    res
}

async fn resolve_api_key(
    svc: &crate::services::Services,
    k: &api_keys::ParsedKey<'_>,
) -> Result<Option<Principal>, ApiError> {
    let Some(row) = app_db::api_keys::find_active(&svc.db, k.key_id).await.map_err(crate::errors::db)? else {
        return Ok(None);
    };
    if !api_keys::verify_secret(&svc.api_key_pepper, k.secret, &row.secret_hash) {
        return Ok(None);
    }
    if let Err(e) = app_db::api_keys::touch(&svc.db, row.id).await {
        tracing::warn!(error = %e, "api key touch failed");
    }
    Ok(Some(Principal::ApiKey {
        key_id: row.id,
        organization_id: row.organization_id,
        scopes: PermissionSet::parse_keys(row.scopes.iter().map(String::as_str)).unwrap_or_default(),
        created_by: row.created_by,
        name: row.name,
    }))
}

/// Request metadata for audit records.
#[derive(Debug, Clone, Default)]
pub struct ReqMeta {
    pub request_id: Option<String>,
    pub ip: Option<std::net::IpAddr>,
    pub user_agent: Option<String>,
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for ReqMeta {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut axum::http::request::Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(ReqMeta {
            request_id: parts.headers.get("x-request-id").and_then(|v| v.to_str().ok()).map(String::from),
            ip: parts.extensions.get::<ClientIp>().and_then(|c| c.0),
            user_agent: parts
                .headers
                .get(header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.chars().take(256).collect()),
        })
    }
}
