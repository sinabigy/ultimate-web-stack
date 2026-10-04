//! Extractors that turn the resolved principal into handler arguments.

use std::collections::HashMap;

use app_authz::{Context, OrgAccess, SystemPermission};
use app_db::{audit::Outcome, orgs};
use app_errors::ApiError;
use axum::{
    extract::{FromRequestParts, Path},
    http::request::Parts,
};
use uuid::Uuid;

use super::{Principal, ReqMeta, UserPrincipal};
use crate::{errors::ResultExt, state::AppState};

/// Any authenticated principal (user session, service account or API key).
pub struct Auth(pub Principal);

impl FromRequestParts<AppState> for Auth {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, _: &AppState) -> Result<Self, ApiError> {
        parts.extensions.get::<Principal>().cloned().map(Auth).ok_or(ApiError::Unauthenticated)
    }
}

/// Optional principal (public endpoints that personalise when signed in).
pub struct MaybeAuth(pub Option<Principal>);

impl FromRequestParts<AppState> for MaybeAuth {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut Parts, _: &AppState) -> Result<Self, Self::Rejection> {
        Ok(MaybeAuth(parts.extensions.get::<Principal>().cloned()))
    }
}

/// A human user with a browser session. Machine credentials are rejected: account and
/// administration endpoints are for people.
pub struct UserAuth(pub UserPrincipal);

impl FromRequestParts<AppState> for UserAuth {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, _: &AppState) -> Result<Self, ApiError> {
        match parts.extensions.get::<Principal>() {
            Some(Principal::User(u)) => Ok(UserAuth(u.clone())),
            Some(_) => Err(ApiError::ForbiddenReason("user_session_required")),
            None => Err(ApiError::Unauthenticated),
        }
    }
}

/// Organisation context from the `{slug}` path segment: the caller's verified access.
/// Non-members get 404 (organisation existence is not revealed) and the probe is audited.
pub struct Org {
    pub access: OrgAccess,
    pub org: orgs::OrgRow,
    pub principal: Principal,
}

impl FromRequestParts<AppState> for Org {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let Auth(principal) = Auth::from_request_parts(parts, state).await?;
        let Path(params) = Path::<HashMap<String, String>>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::BadRequest("missing organization".into()))?;
        let slug = params.get("slug").ok_or(ApiError::BadRequest("missing organization".into()))?;
        let svc = state.svc()?;
        let (org, facts) = match &principal {
            Principal::User(u) => {
                let r = orgs::facts_for_user_by_slug(&svc.db, u.user_id, slug).await.api()?;
                (r.org, r.facts)
            }
            Principal::ApiKey { organization_id, created_by, .. } => {
                let r = orgs::facts_for_user_by_slug(&svc.db, Uuid::nil(), slug).await.api()?;
                if r.org.id != *organization_id {
                    return Err(ApiError::NotFound);
                }
                (r.org, orgs::facts_for_credential(&svc.db, *organization_id, *created_by).await.api()?)
            }
            Principal::Service { organization_id, .. } => {
                let r = orgs::facts_for_user_by_slug(&svc.db, Uuid::nil(), slug).await.api()?;
                if r.org.id != *organization_id {
                    return Err(ApiError::NotFound);
                }
                (r.org, orgs::facts_for_credential(&svc.db, *organization_id, None).await.api()?)
            }
        };
        match svc.authz.org_access(&principal.actor(), &facts) {
            Ok(access) => Ok(Org { access, org, principal }),
            Err(d) => {
                let meta = ReqMeta::from_request_parts(parts, state).await.unwrap_or_default();
                let e = crate::audit::event(
                    Some(&principal),
                    &meta,
                    state.config.auth.store_client_ip,
                    "organization.access_denied",
                    Outcome::Denied,
                )
                .target("organization", org.id)
                .meta(serde_json::json!({"reason": d.to_string()}));
                crate::audit::best_effort(&svc.db, &e).await;
                Err(crate::errors::denied(d))
            }
        }
    }
}

/// System administration guard. Organisation roles never satisfy it; denials are audited.
pub async fn require_system(
    state: &AppState,
    principal: &Principal,
    perm: SystemPermission,
    meta: &ReqMeta,
) -> Result<(), ApiError> {
    let svc = state.svc()?;
    // Mutating platform actions need a recent login (step-up), not just a valid session.
    let mutating =
        matches!(perm, SystemPermission::UsersManage | SystemPermission::OrgsManage | SystemPermission::JobsManage);
    let max_auth_age =
        mutating.then(|| std::time::Duration::from_secs(state.config.auth.reauth_window_minutes.saturating_mul(60)));
    let ctx = Context { require_mfa_for_system: state.config.auth.require_mfa_for_system_admin, max_auth_age };
    match svc.authz.authorize_system(&principal.actor(), perm, &ctx) {
        Ok(()) => Ok(()),
        Err(d) => {
            let e = crate::audit::event(
                Some(principal),
                meta,
                state.config.auth.store_client_ip,
                "admin.access_denied",
                Outcome::Denied,
            )
            .meta(serde_json::json!({"permission": perm.key(), "reason": d.to_string()}));
            crate::audit::best_effort(&svc.db, &e).await;
            Err(crate::errors::denied(d))
        }
    }
}
