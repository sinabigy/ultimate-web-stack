//! Organisation (tenant) APIs. Every handler takes [`Org`], i.e. a server-verified
//! `OrgAccess`; organisation ids are never read from request bodies or query strings.

use app_auth::{api_keys, tokens};
use app_authz::{Permission as P, PermissionSet, Resource};
use app_db::{audit::Outcome, orgs, pagination::Cursor, runs};
use app_domain::{NewRun, RealtimeEvent, Slug, org::validate_org_name};
use app_errors::ApiError;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    audit,
    auth::{
        Principal, ReqMeta,
        extract::{Org, UserAuth},
    },
    dto,
    errors::ResultExt,
    state::AppState,
};

pub(crate) fn ev(state: &AppState, o: &Org, meta: &ReqMeta, action: &'static str) -> app_db::audit::AuditEvent {
    audit::event(Some(&o.principal), meta, state.config.auth.store_client_ip, action, Outcome::Success)
        .org(o.access.org_id())
}

pub(crate) async fn require(
    state: &AppState,
    o: &Org,
    meta: &ReqMeta,
    p: P,
    resource: &Resource,
) -> Result<(), ApiError> {
    match state.svc()?.authz.authorize(&o.access, p, resource) {
        Ok(()) => Ok(()),
        Err(d) => Err(deny(state, o, meta, p, d).await),
    }
}

/// Turn a denial into the API error, recording it first. Every denied non-read permission is
/// evidence of probing or escalation (invariant 10). Reads are not audited here: cross-tenant
/// reads are audited at membership resolution. Use it for structural checks that follow
/// `require` (escalation, credential scopes) so their denials are audited too.
pub(crate) async fn deny(state: &AppState, o: &Org, meta: &ReqMeta, p: P, d: app_authz::Denied) -> ApiError {
    // Reads are the `*:read` permissions (no list to maintain when one is added). Exception: a
    // denied attempt to read the audit trail is itself security evidence.
    let read = p.key().ends_with(":read") && p != P::AuditRead;
    if !read && let Ok(svc) = state.svc() {
        let e =
            audit::event(Some(&o.principal), meta, state.config.auth.store_client_ip, "authz.denied", Outcome::Denied)
                .org(o.access.org_id())
                .meta(json!({"permission": p.key(), "reason": d.to_string()}));
        audit::best_effort(&svc.db, &e).await;
    }
    crate::errors::denied(d)
}

/// Reads go through the configured engine too (RBAC or Cedar), so a policy that forbids a read is
/// enforced. Denied reads are not audited here: cross-tenant reads are audited at membership
/// resolution, and in-tenant read denials are UI navigation, not escalation attempts.
pub(crate) fn require_read(state: &AppState, o: &Org, p: P, resource: &Resource) -> Result<(), ApiError> {
    state.svc()?.authz.authorize(&o.access, p, resource).map_err(crate::errors::denied)
}

// ------------------------------------------------------------------ organisations

pub async fn list_mine(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
) -> Result<Json<dto::ListResponse<orgs::MyOrg>>, ApiError> {
    Ok(Json(dto::ListResponse::new(orgs::list_for_user(&state.svc()?.db, u.user_id).await.api()?)))
}

#[derive(Deserialize)]
pub struct CreateOrg {
    name: String,
    slug: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
    Json(b): Json<CreateOrg>,
) -> Result<(StatusCode, Json<orgs::OrgRow>), ApiError> {
    if !state.config.tenancy.organizations || !state.config.tenancy.allow_org_creation {
        return Err(ApiError::ForbiddenReason("org_creation_disabled"));
    }
    let name = validate_org_name(&b.name).map_err(|e| ApiError::validation(e.field, e.message))?;
    let slug = Slug::parse(b.slug.as_deref().unwrap_or(&Slug::suggest(&name)))
        .map_err(|e| ApiError::validation(e.field, e.message))?;
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    let org = orgs::create_with_owner(&mut tx, &slug, &name, false, u.user_id).await.api()?;
    let p = Principal::User(u);
    let e = audit::event(Some(&p), &meta, state.config.auth.store_client_ip, "organization.created", Outcome::Success)
        .org(org.id)
        .target("organization", org.id);
    app_db::audit::insert(&mut *tx, &e).await.api()?;
    tx.commit().await.api()?;
    Ok((StatusCode::CREATED, Json(org)))
}

fn perm_keys(o: &Org) -> Vec<String> {
    o.access.permissions().keys().into_iter().map(String::from).collect()
}

pub async fn get(State(state): State<AppState>, o: Org) -> Result<Json<dto::OrgDetail>, ApiError> {
    require_read(&state, &o, P::OrgRead, &Resource::Organization)?;
    Ok(Json(dto::OrgDetail {
        role: o.access.role().map(|r| r.key.clone()),
        permissions: perm_keys(&o),
        organization: o.org,
    }))
}

#[derive(Deserialize)]
pub struct UpdateOrg {
    name: Option<String>,
    settings: Option<Value>,
}

pub async fn update(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Json(b): Json<UpdateOrg>,
) -> Result<Json<orgs::OrgRow>, ApiError> {
    // Default deny: every update is authorized, including one with no fields (which would
    // otherwise write and audit without any decision).
    if b.name.is_some() || b.settings.is_none() {
        require(&state, &o, &meta, P::OrgUpdate, &Resource::Organization).await?;
    }
    if b.name.is_none() && b.settings.is_none() {
        return Err(ApiError::validation("body", "nothing to update"));
    }
    if b.settings.is_some() {
        require(&state, &o, &meta, P::SettingsManage, &Resource::Organization).await?;
    }
    let name = match &b.name {
        Some(n) => validate_org_name(n).map_err(|e| ApiError::validation(e.field, e.message))?,
        None => o.org.name.clone(),
    };
    let settings = b.settings.clone().unwrap_or_else(|| o.org.settings.clone());
    if !settings.is_object() || settings.to_string().len() > 32 * 1024 {
        return Err(ApiError::validation("settings", "must be a JSON object under 32 KiB"));
    }
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    let row = orgs::update(&mut *tx, &o.access, &name, &settings).await.api()?;
    let e = ev(&state, &o, &meta, "organization.updated")
        .meta(json!({"name": b.name.is_some(), "settings": b.settings.is_some()}));
    app_db::audit::insert(&mut *tx, &e).await.api()?;
    tx.commit().await.api()?;
    Ok(Json(row))
}

pub async fn delete(State(state): State<AppState>, o: Org, meta: ReqMeta) -> Result<StatusCode, ApiError> {
    require(&state, &o, &meta, P::OrgDelete, &Resource::Organization).await?;
    if let Principal::User(u) = &o.principal
        && OffsetDateTime::now_utc() - u.auth_time
            > time::Duration::minutes(state.config.auth.reauth_window_minutes as i64)
    {
        return Err(ApiError::ForbiddenReason("reauth_required"));
    }
    if o.org.personal {
        return Err(ApiError::ForbiddenReason("personal_organization"));
    }
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    orgs::soft_delete(&mut *tx, &o.access).await.api()?;
    app_db::audit::insert(&mut *tx, &ev(&state, &o, &meta, "organization.deleted").target("organization", o.org.id))
        .await
        .api()?;
    tx.commit().await.api()?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default)]
pub struct DaysQuery {
    days: Option<i32>,
}

/// Organisation dashboard: widgets gated individually by permission.
pub async fn overview(
    State(state): State<AppState>,
    o: Org,
    Query(q): Query<DaysQuery>,
) -> Result<Json<dto::OrgOverview>, ApiError> {
    require_read(&state, &o, P::OrgRead, &Resource::Organization)?;
    let svc = state.svc()?;
    let days = q.days.unwrap_or(14).clamp(1, 90);
    let since = OffsetDateTime::now_utc() - time::Duration::days(i64::from(days));
    let mut widgets = dto::OrgWidgets::default();
    // Each widget is decided by the engine, like the endpoint behind it.
    let allowed = |p: P| require_read(&state, &o, p, &Resource::Organization).is_ok();
    if allowed(P::RunsRead) {
        // Cached aggregate (15s, jittered). The cache key covers only authorization-independent
        // data (org + range); permission gating happens after retrieval, never via the key.
        let key = state.cache.key("org-usage", 1, &[&o.access.org_id().to_string(), &days.to_string()]);
        let (stats, usage): (app_db::runs::RunStats, Vec<dto::UsagePoint>) = state
            .cache
            .get_or_load(&key, std::time::Duration::from_secs(15), || async {
                let stats = runs::stats(&svc.db, &o.access, since).await?;
                let usage = runs::daily_counts(&svc.db, &o.access, days)
                    .await?
                    .into_iter()
                    .map(|(d, ok, err)| dto::UsagePoint { date: d.to_string(), succeeded: ok, failed: err })
                    .collect();
                Ok::<_, app_db::DbError>((stats, usage))
            })
            .await
            .api()?;
        widgets.runs = Some(stats);
        widgets.usage = Some(usage);
    }
    if allowed(P::MembersRead) {
        widgets.members = Some(orgs::list_members(&svc.db, &o.access).await.api()?.len());
    }
    if allowed(P::MembersInvite) {
        widgets.pending_invitations = Some(orgs::list_pending_invitations(&svc.db, &o.access).await.api()?.len());
    }
    if allowed(P::AuditRead) {
        let page = app_db::audit::list(&svc.db, Some(o.access.org_id()), &Default::default(), None, 10).await.api()?;
        widgets.recent_audit = Some(page.items);
    }
    Ok(Json(dto::OrgOverview { permissions: perm_keys(&o), organization: o.org, widgets, days }))
}

// ------------------------------------------------------------------ members

pub async fn list_members(
    State(state): State<AppState>,
    o: Org,
) -> Result<Json<dto::ListResponse<orgs::MemberRow>>, ApiError> {
    require_read(&state, &o, P::MembersRead, &Resource::Organization)?;
    Ok(Json(dto::ListResponse::new(orgs::list_members(&state.svc()?.db, &o.access).await.api()?)))
}

#[derive(Deserialize)]
pub struct ChangeRole {
    role_id: Uuid,
}

pub async fn change_member_role(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Path((_slug, user_id)): Path<(String, Uuid)>,
    Json(b): Json<ChangeRole>,
) -> Result<StatusCode, ApiError> {
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    let owners = orgs::count_owners_for_update(&mut tx, &o.access).await.api()?;
    let current = orgs::member_grant(&mut *tx, &o.access, user_id).await.api()?;
    let new = orgs::role_grant(&mut *tx, &o.access, b.role_id).await.api()?;
    if let Err(d) = svc.authz.authorize_role_change(&o.access, user_id, &current, Some(&new), owners) {
        drop(tx);
        let e = audit::event(
            Some(&o.principal),
            &meta,
            state.config.auth.store_client_ip,
            "role.assign_denied",
            Outcome::Denied,
        )
        .org(o.access.org_id())
        .target("user", user_id)
        .meta(json!({"to": new.key, "reason": d.to_string()}));
        audit::best_effort(&svc.db, &e).await;
        return Err(crate::errors::denied(d));
    }
    orgs::set_member_role(&mut *tx, &o.access, user_id, b.role_id).await.api()?;
    let e = ev(&state, &o, &meta, "role.assigned")
        .target("user", user_id)
        .meta(json!({"from": current.key, "to": new.key}));
    app_db::audit::insert(&mut *tx, &e).await.api()?;
    tx.commit().await.api()?;
    // Privilege change: the member's sessions keep working, but authorization is
    // re-evaluated on every request from the database, so the new role applies immediately.
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_member(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Path((_slug, user_id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    let owners = orgs::count_owners_for_update(&mut tx, &o.access).await.api()?;
    let current = orgs::member_grant(&mut *tx, &o.access, user_id).await.api()?;
    if let Err(d) = svc.authz.authorize_role_change(&o.access, user_id, &current, None, owners) {
        drop(tx);
        let e = audit::event(
            Some(&o.principal),
            &meta,
            state.config.auth.store_client_ip,
            "organization.member_remove_denied",
            Outcome::Denied,
        )
        .org(o.access.org_id())
        .target("user", user_id)
        .meta(json!({"reason": d.to_string()}));
        audit::best_effort(&svc.db, &e).await;
        return Err(crate::errors::denied(d));
    }
    orgs::remove_member(&mut *tx, &o.access, user_id).await.api()?;
    let self_leave = o.principal.id() == user_id;
    let action = if self_leave { "organization.member_left" } else { "organization.member_removed" };
    app_db::audit::insert(
        &mut *tx,
        &ev(&state, &o, &meta, action).target("user", user_id).meta(json!({"role": current.key})),
    )
    .await
    .api()?;
    tx.commit().await.api()?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn leave(State(state): State<AppState>, o: Org, meta: ReqMeta) -> Result<StatusCode, ApiError> {
    let Principal::User(u) = &o.principal else { return Err(ApiError::ForbiddenReason("user_session_required")) };
    if o.org.personal {
        return Err(ApiError::ForbiddenReason("personal_organization"));
    }
    let uid = u.user_id;
    remove_member(State(state), o, meta, Path((String::new(), uid))).await
}

// ------------------------------------------------------------------ invitations

pub async fn list_invitations(
    State(state): State<AppState>,
    o: Org,
) -> Result<Json<dto::ListResponse<orgs::InvitationRow>>, ApiError> {
    require_read(&state, &o, P::MembersInvite, &Resource::Organization)?;
    Ok(Json(dto::ListResponse::new(orgs::list_pending_invitations(&state.svc()?.db, &o.access).await.api()?)))
}

#[derive(Deserialize)]
pub struct Invite {
    email: String,
    role_id: Uuid,
}

pub async fn invite(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Json(b): Json<Invite>,
) -> Result<(StatusCode, Json<dto::InviteCreated>), ApiError> {
    let Principal::User(u) = &o.principal else { return Err(ApiError::ForbiddenReason("user_session_required")) };
    if o.org.personal {
        return Err(ApiError::ForbiddenReason("personal_organization"));
    }
    let email = app_domain::Email::parse(&b.email).map_err(|e| ApiError::validation(e.field, e.message))?;
    let svc = state.svc()?;
    let role = orgs::role_grant(&svc.db, &o.access, b.role_id).await.api()?;
    require(&state, &o, &meta, P::MembersInvite, &Resource::Organization).await?;
    if let Err(d) = svc.rbac.authorize_invite(&o.access, &role) {
        return Err(deny(&state, &o, &meta, P::MembersInvite, d).await);
    }
    let token = tokens::new_token();
    let expires = OffsetDateTime::now_utc() + time::Duration::hours(state.config.tenancy.invitation_ttl_hours as i64);
    let mut tx = svc.db.begin().await.api()?;
    let id = orgs::create_invitation(
        &mut *tx,
        &o.access,
        email.as_str(),
        role.role_id,
        &tokens::token_hash(&token),
        u.user_id,
        expires,
    )
    .await
    .api()?;
    let e = ev(&state, &o, &meta, "organization.member_invited")
        .target("invitation", id)
        .meta(json!({"email": email.as_str(), "role": role.key}));
    app_db::audit::insert(&mut *tx, &e).await.api()?;
    tx.commit().await.api()?;
    // Email delivery hook: deployments wire a mailer here; the link is returned once so an
    // admin can share it manually when no mailer is configured.
    let link = format!("{}/invitations/{token}", state.config.auth.public_origin.trim_end_matches('/'));
    tracing::info!(invitation = %id, "invitation created (deliver the link via the configured mailer)");
    Ok((StatusCode::CREATED, Json(dto::InviteCreated { id, link, expires_at: dto::rfc3339(expires) })))
}

pub async fn revoke_invitation(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Path((_slug, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    require(&state, &o, &meta, P::MembersInvite, &Resource::Organization).await?;
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    orgs::revoke_invitation(&mut *tx, &o.access, id).await.api()?;
    app_db::audit::insert(&mut *tx, &ev(&state, &o, &meta, "organization.invitation_revoked").target("invitation", id))
        .await
        .api()?;
    tx.commit().await.api()?;
    Ok(StatusCode::NO_CONTENT)
}

/// Invitation preview for the accept page (signed-in users only; reveals org name + role).
pub async fn invitation_preview(
    State(state): State<AppState>,
    UserAuth(_u): UserAuth,
    Path(token): Path<String>,
) -> Result<Json<dto::InvitationView>, ApiError> {
    if !tokens::plausible_token(&token) {
        return Err(ApiError::NotFound);
    }
    let inv = orgs::find_invitation(&state.svc()?.db, &tokens::token_hash(&token)).await.api()?;
    Ok(Json(dto::InvitationView {
        organization: dto::OrgRef { slug: inv.organization_slug, name: inv.organization_name },
        role: inv.role_key,
        email: inv.email,
        expires_at: dto::rfc3339(inv.expires_at),
    }))
}

/// Accept: the signed-in user's *verified* email must equal the invited address, so a
/// forwarded or leaked link cannot be used by someone else.
pub async fn accept_invitation(
    State(state): State<AppState>,
    UserAuth(u): UserAuth,
    meta: ReqMeta,
    Path(token): Path<String>,
) -> Result<Json<dto::InvitationAccepted>, ApiError> {
    if !tokens::plausible_token(&token) {
        return Err(ApiError::NotFound);
    }
    let svc = state.svc()?;
    let inv = orgs::find_invitation(&svc.db, &tokens::token_hash(&token)).await.api()?;
    if !u.email_verified {
        return Err(ApiError::ForbiddenReason("email_unverified"));
    }
    if !inv.email.eq_ignore_ascii_case(&u.email) {
        let p = Principal::User(u.clone());
        let e = audit::event(
            Some(&p),
            &meta,
            state.config.auth.store_client_ip,
            "organization.invitation_mismatch",
            Outcome::Denied,
        )
        .org(inv.organization_id)
        .target("invitation", inv.id);
        audit::best_effort(&svc.db, &e).await;
        return Err(ApiError::ForbiddenReason("invitation_email_mismatch"));
    }
    let mut tx = svc.db.begin().await.api()?;
    let org_id = orgs::accept_invitation(&mut tx, inv.id, u.user_id).await.api()?;
    let p = Principal::User(u.clone());
    let e =
        audit::event(Some(&p), &meta, state.config.auth.store_client_ip, "organization.member_added", Outcome::Success)
            .org(org_id)
            .target("user", u.user_id)
            .meta(json!({"role": inv.role_key, "via": "invitation"}));
    app_db::audit::insert(&mut *tx, &e).await.api()?;
    tx.commit().await.api()?;
    Ok(Json(dto::InvitationAccepted {
        organization: dto::OrgRef { slug: inv.organization_slug, name: inv.organization_name },
        role: inv.role_key,
    }))
}

// ------------------------------------------------------------------ teams

pub async fn list_teams(
    State(state): State<AppState>,
    o: Org,
) -> Result<Json<dto::ListResponse<orgs::TeamRow>>, ApiError> {
    require_read(&state, &o, P::TeamsRead, &Resource::Organization)?;
    Ok(Json(dto::ListResponse::new(orgs::list_teams(&state.svc()?.db, &o.access).await.api()?)))
}

#[derive(Deserialize)]
pub struct CreateTeam {
    name: String,
    #[serde(default)]
    description: String,
}

pub async fn create_team(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Json(b): Json<CreateTeam>,
) -> Result<(StatusCode, Json<dto::Created>), ApiError> {
    require(&state, &o, &meta, P::TeamsManage, &Resource::Organization).await?;
    let name = validate_org_name(&b.name).map_err(|e| ApiError::validation("name", e.message))?;
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    let id =
        orgs::create_team(&mut *tx, &o.access, &name, b.description.chars().take(500).collect::<String>().as_str())
            .await
            .api()?;
    app_db::audit::insert(&mut *tx, &ev(&state, &o, &meta, "team.created").target("team", id)).await.api()?;
    tx.commit().await.api()?;
    Ok((StatusCode::CREATED, Json(dto::Created { id })))
}

pub async fn delete_team(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Path((_s, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    require(&state, &o, &meta, P::TeamsManage, &Resource::Organization).await?;
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    orgs::delete_team(&mut *tx, &o.access, id).await.api()?;
    app_db::audit::insert(&mut *tx, &ev(&state, &o, &meta, "team.deleted").target("team", id)).await.api()?;
    tx.commit().await.api()?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct TeamMember {
    user_id: Uuid,
}

pub async fn add_team_member(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Path((_s, team)): Path<(String, Uuid)>,
    Json(b): Json<TeamMember>,
) -> Result<StatusCode, ApiError> {
    require(&state, &o, &meta, P::TeamsManage, &Resource::Organization).await?;
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    orgs::add_team_member(&mut *tx, &o.access, team, b.user_id).await.api()?;
    app_db::audit::insert(
        &mut *tx,
        &ev(&state, &o, &meta, "team.member_added").target("team", team).meta(json!({"user_id": b.user_id})),
    )
    .await
    .api()?;
    tx.commit().await.api()?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_team_member(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Path((_s, team, user)): Path<(String, Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    require(&state, &o, &meta, P::TeamsManage, &Resource::Organization).await?;
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    orgs::remove_team_member(&mut *tx, &o.access, team, user).await.api()?;
    app_db::audit::insert(
        &mut *tx,
        &ev(&state, &o, &meta, "team.member_removed").target("team", team).meta(json!({"user_id": user})),
    )
    .await
    .api()?;
    tx.commit().await.api()?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_team_members(
    State(state): State<AppState>,
    o: Org,
    Path((_s, team)): Path<(String, Uuid)>,
) -> Result<Json<dto::ListResponse<Uuid>>, ApiError> {
    require_read(&state, &o, P::TeamsRead, &Resource::Organization)?;
    Ok(Json(dto::ListResponse::new(orgs::list_team_members(&state.svc()?.db, &o.access, team).await.api()?)))
}

// ------------------------------------------------------------------ roles

pub async fn list_roles(
    State(state): State<AppState>,
    o: Org,
) -> Result<Json<dto::ListResponse<orgs::RoleRow>>, ApiError> {
    require_read(&state, &o, P::RolesRead, &Resource::Organization)?;
    Ok(Json(dto::ListResponse::new(orgs::list_roles(&state.svc()?.db, &o.access).await.api()?)))
}

#[derive(Deserialize)]
pub struct CreateRole {
    key: String,
    name: String,
    #[serde(default)]
    description: String,
    permissions: Vec<String>,
}

pub async fn create_role(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Json(b): Json<CreateRole>,
) -> Result<(StatusCode, Json<dto::Created>), ApiError> {
    require(&state, &o, &meta, P::RolesManage, &Resource::Organization).await?;
    let key = b.key.trim().to_ascii_lowercase();
    if key.is_empty()
        || key.len() > 40
        || !key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(ApiError::validation("key", "lowercase letters, digits and underscores (max 40)"));
    }
    if app_domain::OrgRole::parse(&key).is_some() {
        return Err(ApiError::validation("key", "conflicts with a built-in role"));
    }
    let name = validate_org_name(&b.name).map_err(|e| ApiError::validation("name", e.message))?;
    let perms = PermissionSet::parse_keys(b.permissions.iter().map(String::as_str))
        .map_err(|e| ApiError::validation("permissions", e))?;
    if !perms.is_subset_of(&P::assignable_to_custom_roles()) {
        return Err(ApiError::validation("permissions", "owner-only permissions cannot be granted by custom roles"));
    }
    if !perms.is_subset_of(o.access.permissions()) {
        return Err(ApiError::ForbiddenReason("escalation"));
    }
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    let id = orgs::create_custom_role(&mut tx, &o.access, &key, &name, &b.description, &perms).await.api()?;
    app_db::audit::insert(
        &mut *tx,
        &ev(&state, &o, &meta, "role.created").target("role", id).meta(json!({"key": key, "permissions": perms})),
    )
    .await
    .api()?;
    tx.commit().await.api()?;
    Ok((StatusCode::CREATED, Json(dto::Created { id })))
}

pub async fn delete_role(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Path((_s, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    require(&state, &o, &meta, P::RolesManage, &Resource::Organization).await?;
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    orgs::delete_custom_role(&mut *tx, &o.access, id).await.api()?;
    app_db::audit::insert(&mut *tx, &ev(&state, &o, &meta, "role.deleted").target("role", id)).await.api()?;
    tx.commit().await.api()?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn permission_catalog() -> Json<dto::ListResponse<dto::PermissionInfo>> {
    Json(dto::ListResponse::new(
        P::ALL
            .iter()
            .map(|p| dto::PermissionInfo {
                key: p.key().into(),
                description: p.description().into(),
                owner_only: P::owner_only().contains(*p),
                credential_assignable: P::assignable_to_credentials().contains(*p),
            })
            .collect(),
    ))
}

// ------------------------------------------------------------------ audit

#[derive(Deserialize, Default)]
pub struct AuditQuery {
    pub action: Option<String>,
    pub actor_id: Option<Uuid>,
    pub outcome: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub since: Option<i64>,
    pub until: Option<i64>,
}

impl AuditQuery {
    pub fn filter(&self) -> Result<(app_db::audit::AuditFilter, Option<Cursor>, i64), ApiError> {
        let ts = |s: Option<i64>| s.and_then(|v| OffsetDateTime::from_unix_timestamp(v).ok());
        let cursor = match &self.cursor {
            Some(c) => Some(Cursor::decode(c).ok_or(ApiError::BadRequest("invalid cursor".into()))?),
            None => None,
        };
        Ok((
            app_db::audit::AuditFilter {
                action_prefix: self.action.clone(),
                actor_id: self.actor_id,
                outcome: self.outcome.clone().filter(|o| matches!(o.as_str(), "success" | "denied" | "failure")),
                since: ts(self.since),
                until: ts(self.until),
            },
            cursor,
            app_db::pagination::clamp_limit(self.limit, 50, 200),
        ))
    }
}

pub async fn audit_log(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Query(q): Query<AuditQuery>,
) -> Result<Json<app_db::pagination::Page<app_db::audit::AuditRow>>, ApiError> {
    require(&state, &o, &meta, P::AuditRead, &Resource::Organization).await?;
    let (filter, cursor, limit) = q.filter()?;
    Ok(Json(app_db::audit::list(&state.svc()?.db, Some(o.access.org_id()), &filter, cursor, limit).await.api()?))
}

// ------------------------------------------------------------------ API keys & service clients

pub async fn list_api_keys(
    State(state): State<AppState>,
    o: Org,
) -> Result<Json<dto::ListResponse<app_db::api_keys::ApiKeyRow>>, ApiError> {
    require_read(&state, &o, P::ApiKeysRead, &Resource::Organization)?;
    Ok(Json(dto::ListResponse::new(app_db::api_keys::list(&state.svc()?.db, &o.access).await.api()?)))
}

#[derive(Deserialize)]
pub struct CreateKey {
    name: String,
    scopes: Vec<String>,
    expires_in_days: Option<i64>,
    #[serde(default)]
    test: bool,
}

pub async fn create_api_key(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Json(b): Json<CreateKey>,
) -> Result<(StatusCode, Json<dto::ApiKeyCreated>), ApiError> {
    let Principal::User(u) = &o.principal else { return Err(ApiError::ForbiddenReason("user_session_required")) };
    let scopes = PermissionSet::parse_keys(b.scopes.iter().map(String::as_str))
        .map_err(|e| ApiError::validation("scopes", e))?;
    let svc = state.svc()?;
    require(&state, &o, &meta, P::ApiKeysManage, &Resource::Organization).await?;
    if let Err(d) = svc.rbac.authorize_credential_scopes(&o.access, &scopes) {
        return Err(deny(&state, &o, &meta, P::ApiKeysManage, d).await);
    }
    let name = validate_org_name(&b.name).map_err(|e| ApiError::validation("name", e.message))?;
    let expires = b.expires_in_days.map(|d| OffsetDateTime::now_utc() + time::Duration::days(d.clamp(1, 3650)));
    let generated = api_keys::generate(&state.config.auth.api_key_prefix, !b.test, &svc.api_key_pepper);
    let scope_keys: Vec<String> = scopes.keys().into_iter().map(String::from).collect();
    let mut tx = svc.db.begin().await.api()?;
    let id = app_db::api_keys::insert(
        &mut *tx,
        &o.access,
        &app_db::api_keys::NewApiKey {
            key_id: &generated.key_id,
            secret_hash: &generated.secret_hash,
            name: &name,
            created_by: u.user_id,
            scopes: &scope_keys,
            expires_at: expires,
            rotated_from: None,
        },
    )
    .await
    .api()?;
    app_db::audit::insert(
        &mut *tx,
        &ev(&state, &o, &meta, "api_key.created")
            .target("api_key", id)
            .meta(json!({"key_id": generated.key_id, "scopes": scope_keys})),
    )
    .await
    .api()?;
    tx.commit().await.api()?;
    // The plaintext key is returned exactly once and never stored.
    Ok((
        StatusCode::CREATED,
        Json(dto::ApiKeyCreated {
            id,
            key: generated.plaintext,
            key_id: generated.key_id,
            expires_at: expires.map(dto::rfc3339),
        }),
    ))
}

pub async fn revoke_api_key(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Path((_s, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    require(&state, &o, &meta, P::ApiKeysManage, &Resource::Organization).await?;
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    app_db::api_keys::revoke(&mut *tx, &o.access, id).await.api()?;
    app_db::audit::insert(&mut *tx, &ev(&state, &o, &meta, "api_key.revoked").target("api_key", id)).await.api()?;
    tx.commit().await.api()?;
    Ok(StatusCode::NO_CONTENT)
}

/// Rotate: issue a replacement with the same name and scopes; the old key keeps working for
/// 24 hours so deployments can switch without downtime.
pub async fn rotate_api_key(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Path((_s, id)): Path<(String, Uuid)>,
) -> Result<(StatusCode, Json<dto::ApiKeyRotated>), ApiError> {
    let Principal::User(u) = &o.principal else { return Err(ApiError::ForbiddenReason("user_session_required")) };
    let svc = state.svc()?;
    let old = app_db::api_keys::get(&svc.db, &o.access, id).await.api()?;
    if old.revoked_at.is_some() {
        return Err(ApiError::Conflict("key is revoked".into()));
    }
    let scopes = PermissionSet::parse_keys(old.scopes.iter().map(String::as_str)).unwrap_or_default();
    require(&state, &o, &meta, P::ApiKeysManage, &Resource::Organization).await?;
    if let Err(d) = svc.rbac.authorize_credential_scopes(&o.access, &scopes) {
        return Err(deny(&state, &o, &meta, P::ApiKeysManage, d).await);
    }
    let generated = api_keys::generate(&state.config.auth.api_key_prefix, true, &svc.api_key_pepper);
    let mut tx = svc.db.begin().await.api()?;
    let new_id = app_db::api_keys::insert(
        &mut *tx,
        &o.access,
        &app_db::api_keys::NewApiKey {
            key_id: &generated.key_id,
            secret_hash: &generated.secret_hash,
            name: &old.name,
            created_by: u.user_id,
            scopes: &old.scopes,
            expires_at: old.expires_at,
            rotated_from: Some(old.id),
        },
    )
    .await
    .api()?;
    let overlap_until = OffsetDateTime::now_utc() + time::Duration::hours(24);
    app_db::api_keys::expire_at(&mut *tx, &o.access, old.id, overlap_until).await.api()?;
    app_db::audit::insert(
        &mut *tx,
        &ev(&state, &o, &meta, "api_key.rotated").target("api_key", new_id).meta(json!({"rotated_from": old.id})),
    )
    .await
    .api()?;
    tx.commit().await.api()?;
    Ok((
        StatusCode::CREATED,
        Json(dto::ApiKeyRotated {
            id: new_id,
            key: generated.plaintext,
            old_key_expires_at: dto::rfc3339(overlap_until),
        }),
    ))
}

#[derive(Deserialize)]
pub struct RegisterService {
    subject: String,
    name: String,
    scopes: Vec<String>,
}

pub async fn register_service_client(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Json(b): Json<RegisterService>,
) -> Result<(StatusCode, Json<dto::Created>), ApiError> {
    let Principal::User(u) = &o.principal else { return Err(ApiError::ForbiddenReason("user_session_required")) };
    let scopes = PermissionSet::parse_keys(b.scopes.iter().map(String::as_str))
        .map_err(|e| ApiError::validation("scopes", e))?;
    let svc = state.svc()?;
    require(&state, &o, &meta, P::ApiKeysManage, &Resource::Organization).await?;
    if let Err(d) = svc.rbac.authorize_credential_scopes(&o.access, &scopes) {
        return Err(deny(&state, &o, &meta, P::ApiKeysManage, d).await);
    }
    if b.subject.trim().is_empty() || b.subject.len() > 255 {
        return Err(ApiError::validation("subject", "required"));
    }
    let keys: Vec<String> = scopes.keys().into_iter().map(String::from).collect();
    let mut tx = svc.db.begin().await.api()?;
    let id =
        app_db::api_keys::register_service_client(&mut *tx, &o.access, b.subject.trim(), &b.name, &keys, u.user_id)
            .await
            .api()?;
    app_db::audit::insert(
        &mut *tx,
        &ev(&state, &o, &meta, "service_client.registered").target("service_client", id).meta(json!({"scopes": keys})),
    )
    .await
    .api()?;
    tx.commit().await.api()?;
    Ok((StatusCode::CREATED, Json(dto::Created { id })))
}

// ------------------------------------------------------------------ billing hook

pub async fn billing(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
) -> Result<Json<dto::BillingResponse>, ApiError> {
    require(&state, &o, &meta, P::BillingRead, &Resource::Organization).await?;
    Ok(Json(dto::BillingResponse {
        plan: o.org.billing_plan.clone(),
        has_customer: o.org.billing_customer_ref.is_some(),
        provider: None,
        note: "Billing is a hook: connect a provider (e.g. Stripe) by implementing the billing adapter; see docs/multitenancy/billing.md".into(),
    }))
}

// ------------------------------------------------------------------ runs (example domain)

#[derive(Deserialize, Default)]
pub struct RunsQuery {
    status: Option<String>,
    cursor: Option<String>,
    limit: Option<i64>,
}

pub async fn list_runs(
    State(state): State<AppState>,
    o: Org,
    Query(q): Query<RunsQuery>,
) -> Result<Json<app_db::pagination::Page<app_domain::Run>>, ApiError> {
    require_read(&state, &o, P::RunsRead, &Resource::Organization)?;
    let cursor = match &q.cursor {
        Some(c) => Some(Cursor::decode(c).ok_or(ApiError::BadRequest("invalid cursor".into()))?),
        None => None,
    };
    let status = q.status.as_deref().filter(|s| app_domain::RunStatus::parse(s).is_some());
    let page =
        runs::list(&state.svc()?.db, &o.access, status, cursor, app_db::pagination::clamp_limit(q.limit, 25, 100))
            .await
            .api()?;
    Ok(Json(page))
}

pub async fn create_run(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Json(b): Json<NewRun>,
) -> Result<(StatusCode, Json<app_domain::Run>), ApiError> {
    require(&state, &o, &meta, P::RunsCreate, &Resource::Organization).await?;
    let new = b.validate().map_err(|errs| {
        ApiError::Validation(
            errs.into_iter().map(|e| app_errors::FieldError { field: e.field.into(), message: e.message }).collect(),
        )
    })?;
    if !state.config.providers.definitions.contains_key(&new.provider) {
        return Err(ApiError::validation("provider", "unknown provider"));
    }
    let svc = state.svc()?;
    let owner = o.principal.actor().user_id();
    let mut tx = svc.db.begin().await.api()?;
    let run = runs::create(&mut *tx, &o.access, owner, &new).await.api()?;
    let key = format!("run:{}", run.id);
    app_db::jobs::enqueue(
        &mut *tx,
        &app_db::jobs::NewJob {
            queue: "runs",
            kind: "execute_run",
            payload: json!({"run_id": run.id, "organization_id": run.organization_id}),
            priority: 0,
            max_attempts: state.config.jobs.max_attempts,
            run_at: None,
            idempotency_key: Some(&key),
            organization_id: Some(run.organization_id),
            trace_context: Some(crate::audit::job_trace_context(meta.request_id.as_deref())),
        },
    )
    .await
    .api()?;
    app_db::audit::insert(&mut *tx, &ev(&state, &o, &meta, "run.created").target("run", run.id)).await.api()?;
    tx.commit().await.api()?;
    state.analytics.record(
        app_analytics::EventRow::new("run_created", run.organization_id)
            .user(owner)
            .request_id(meta.request_id.as_deref())
            .value(f64::from(run.requested)),
    );
    state
        .events
        .publish(RealtimeEvent::RunCreated {
            run_id: run.id,
            organization_id: run.organization_id,
            requested: run.requested,
        })
        .await;
    Ok((StatusCode::CREATED, Json(run)))
}

pub async fn get_run(
    State(state): State<AppState>,
    o: Org,
    Path((_s, id)): Path<(String, Uuid)>,
) -> Result<Json<app_domain::Run>, ApiError> {
    require_read(&state, &o, P::RunsRead, &Resource::Organization)?;
    Ok(Json(runs::get(&state.svc()?.db, &o.access, id).await.api()?))
}

pub async fn delete_run(
    State(state): State<AppState>,
    o: Org,
    meta: ReqMeta,
    Path((_s, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let svc = state.svc()?;
    let run = runs::get(&svc.db, &o.access, id).await.api()?;
    let owner = (!run.owner_id.is_nil()).then_some(run.owner_id);
    require(&state, &o, &meta, P::RunsManageOwn, &Resource::Owned { owner_id: owner }).await?;
    let mut tx = svc.db.begin().await.api()?;
    runs::delete(&mut *tx, &o.access, id).await.api()?;
    app_db::audit::insert(&mut *tx, &ev(&state, &o, &meta, "run.deleted").target("run", id)).await.api()?;
    tx.commit().await.api()?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct AnalyticsParams {
    days: Option<u32>,
}

/// Daily run activity from ClickHouse. 404 when the analytics module is off.
pub async fn run_analytics(
    State(state): State<AppState>,
    o: Org,
    Query(q): Query<AnalyticsParams>,
) -> Result<Json<dto::RunAnalytics>, ApiError> {
    require_read(&state, &o, P::RunsRead, &Resource::Organization)?;
    let Some(analytics) = state.analytics_query.clone() else { return Err(ApiError::NotFound) };
    let days = q.days.unwrap_or(30).clamp(1, 365);
    let (created, finished) = tokio::try_join!(
        analytics.daily(&o.access, "run_created", days),
        analytics.daily(&o.access, "run_finished", days)
    )
    .map_err(|e| {
        tracing::warn!(error = %e, "analytics query failed");
        ApiError::Unavailable("analytics")
    })?;
    let points = |v: Vec<app_analytics::DailyPoint>| {
        v.into_iter()
            .map(|p| dto::AnalyticsPoint { day: p.day.to_string(), events: p.events, value: p.value })
            .collect::<Vec<_>>()
    };
    Ok(Json(dto::RunAnalytics { days, created: points(created), finished: points(finished) }))
}
