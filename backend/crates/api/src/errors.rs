//! Mapping domain/infrastructure errors to API errors (the orphan rule prevents `From` impls).

use app_authz::Denied;
use app_db::DbError;
use app_errors::ApiError;

pub fn db(e: DbError) -> ApiError {
    match e {
        DbError::NotFound => ApiError::NotFound,
        DbError::Conflict(c) => ApiError::Conflict(conflict_message(&c).to_string()),
        DbError::Unavailable(_) => ApiError::Unavailable("database"),
        DbError::Other(e) => ApiError::internal(e),
    }
}

/// Human messages for constraint violations clients can cause; never leak other names.
fn conflict_message(constraint: &str) -> &'static str {
    match constraint {
        "organizations_slug_key" => "that organization URL is taken",
        "invitations_pending_email_idx" => "an invitation for this email is already pending",
        "roles_scope_key" => "a role with this key already exists",
        "teams_organization_id_name_key" => "a team with this name already exists",
        "organization_memberships_pkey" => "already a member",
        "memberships_role_scope" | "roles_builtin_global" => "role not available in this organization",
        "organization_memberships_role_id_fkey" => "role is still assigned to members",
        "team_members_organization_id_user_id_fkey" => "user is not a member of this organization",
        _ => "conflicting change",
    }
}

pub fn denied(d: Denied) -> ApiError {
    match d {
        // Hide existence of organisations the caller cannot access.
        Denied::NotMember => ApiError::NotFound,
        Denied::MissingPermission(_) => ApiError::ForbiddenReason("missing_permission"),
        Denied::SystemPrivilegeRequired => ApiError::ForbiddenReason("system_privilege_required"),
        Denied::MfaRequired => ApiError::ForbiddenReason("mfa_required"),
        Denied::ReauthenticationRequired => ApiError::ForbiddenReason("reauth_required"),
        Denied::InactiveAccount => ApiError::ForbiddenReason("account_inactive"),
        Denied::Escalation => ApiError::ForbiddenReason("escalation"),
        Denied::LastOwner => ApiError::ForbiddenReason("last_owner"),
        Denied::Forbidden(_) => ApiError::Forbidden,
    }
}

pub trait ResultExt<T> {
    fn api(self) -> Result<T, ApiError>;
}

impl<T> ResultExt<T> for Result<T, DbError> {
    fn api(self) -> Result<T, ApiError> {
        self.map_err(db)
    }
}

impl<T> ResultExt<T> for Result<T, Denied> {
    fn api(self) -> Result<T, ApiError> {
        self.map_err(denied)
    }
}

impl<T> ResultExt<T> for Result<T, sqlx::Error> {
    fn api(self) -> Result<T, ApiError> {
        self.map_err(|e| db(DbError::from(e)))
    }
}
