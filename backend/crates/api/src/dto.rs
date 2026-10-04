//! Response types of the HTTP API. Every type here derives `ts_rs::TS` and is exported to
//! `frontend/src/api/generated/` (`./dev types`), so the frontend compiles against exactly what
//! the backend serialises. Timestamps are RFC 3339 strings; counts are JSON numbers.

use app_auth::idp_admin::SecurityOverview;
use app_db::{
    audit::AuditRow,
    jobs::{JobRow, QueueStats},
    notifications::NotificationRow,
    orgs::{MyOrg, OrgRow},
    runs::RunStats,
    sessions::SessionSummary,
    users::SystemCounts,
};
use app_domain::SystemRole;
use serde::Serialize;
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ListResponse<T: TS> {
    pub items: Vec<T>,
}

impl<T: TS> ListResponse<T> {
    pub fn new(items: Vec<T>) -> Self {
        Self { items }
    }
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct PagedResponse<T: TS> {
    pub items: Vec<T>,
    #[ts(type = "number")]
    pub total: i64,
    #[ts(type = "number")]
    pub page: i64,
    #[ts(type = "number")]
    pub per_page: i64,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct Created {
    pub id: Uuid,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct CountResponse {
    #[ts(type = "number")]
    pub count: u64,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct OkResponse {
    pub ok: bool,
}

// ------------------------------------------------------------------ session / auth

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct SessionUser {
    pub id: Uuid,
    pub email: String,
    pub email_verified: bool,
    pub display_name: String,
    pub system_role: SystemRole,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct SessionMeta {
    pub id: Uuid,
    pub mfa: bool,
    pub amr: Vec<String>,
    /// Unix seconds of the last authentication at the identity provider.
    #[ts(type = "number")]
    pub auth_time: i64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct Features {
    pub organizations: bool,
    pub org_creation: bool,
    pub admin: bool,
    pub auth_profile: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LoginConfig {
    pub provider: String,
    pub passkey: bool,
    pub password: bool,
    pub social: Vec<String>,
    pub enterprise_sso: bool,
    pub registration: bool,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct SessionResponse {
    pub authenticated: bool,
    pub csrf_token: Option<String>,
    pub user: Option<SessionUser>,
    pub session: Option<SessionMeta>,
    pub organizations: Vec<MyOrg>,
    #[ts(type = "number")]
    pub unread_notifications: i64,
    pub features: Features,
    pub login: LoginConfig,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct LogoutResponse {
    pub redirect: String,
}

// ------------------------------------------------------------------ account

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct IdentityRef {
    pub provider: String,
    pub subject: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ProfileResponse {
    pub id: Uuid,
    pub email: String,
    pub email_verified: bool,
    pub display_name: String,
    pub avatar_url: Option<String>,
    #[ts(type = "Record<string, unknown>")]
    pub preferences: serde_json::Value,
    pub system_role: String,
    #[ts(type = "string")]
    pub created_at: String,
    #[ts(type = "string | null")]
    pub last_login_at: Option<String>,
    pub identity: IdentityRef,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ProfileUpdated {
    pub display_name: String,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct IdpState {
    /// False when the identity provider's management API could not be reached.
    pub available: bool,
    pub overview: Option<SecurityOverview>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct SecurityResponse {
    pub session: SessionMeta,
    pub identity_provider: IdpState,
    pub events: Vec<AuditRow>,
    #[ts(type = "number")]
    pub reauth_window_minutes: u64,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct SessionItem {
    #[serde(flatten)]
    pub session: SessionSummary,
    pub current: bool,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct NotificationsResponse {
    pub items: Vec<NotificationRow>,
    #[ts(type = "number")]
    pub unread: i64,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct AccountWidget {
    pub display_name: String,
    pub email: String,
    pub email_verified: bool,
    #[ts(type = "number")]
    pub organizations: usize,
    pub system_role: SystemRole,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct SecurityWidget {
    pub mfa_this_session: bool,
    #[ts(type = "number")]
    pub active_sessions: usize,
    pub email_verified: bool,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct NotificationsWidget {
    #[ts(type = "number")]
    pub unread: i64,
    pub recent: Vec<NotificationRow>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct DashboardWidgets {
    pub account: AccountWidget,
    pub security: SecurityWidget,
    pub notifications: NotificationsWidget,
    pub activity: Vec<AuditRow>,
    pub organizations: Vec<MyOrg>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct DashboardResponse {
    pub widgets: DashboardWidgets,
}

// ------------------------------------------------------------------ organisations

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct OrgDetail {
    pub organization: OrgRow,
    pub role: Option<String>,
    pub permissions: Vec<String>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct UsagePoint {
    pub date: String,
    #[ts(type = "number")]
    pub succeeded: i64,
    #[ts(type = "number")]
    pub failed: i64,
}

#[derive(Debug, Default, Serialize, TS)]
#[ts(export)]
pub struct OrgWidgets {
    pub runs: Option<RunStats>,
    pub usage: Option<Vec<UsagePoint>>,
    #[ts(type = "number | null")]
    pub members: Option<usize>,
    #[ts(type = "number | null")]
    pub pending_invitations: Option<usize>,
    pub recent_audit: Option<Vec<AuditRow>>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct OrgOverview {
    pub organization: OrgRow,
    pub permissions: Vec<String>,
    pub widgets: OrgWidgets,
    pub days: i32,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct OrgRef {
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct InviteCreated {
    pub id: Uuid,
    /// One-time link; deliver via the configured mailer or share manually.
    pub link: String,
    #[ts(type = "string")]
    pub expires_at: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct InvitationView {
    pub organization: OrgRef,
    pub role: String,
    pub email: String,
    #[ts(type = "string")]
    pub expires_at: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct InvitationAccepted {
    pub organization: OrgRef,
    pub role: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct PermissionInfo {
    pub key: String,
    pub description: String,
    pub owner_only: bool,
    pub credential_assignable: bool,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ApiKeyCreated {
    pub id: Uuid,
    /// The full key. Returned exactly once; it is never stored or shown again.
    pub key: String,
    pub key_id: String,
    #[ts(type = "string | null")]
    pub expires_at: Option<String>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ApiKeyRotated {
    pub id: Uuid,
    pub key: String,
    #[ts(type = "string")]
    pub old_key_expires_at: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct BillingResponse {
    pub plan: String,
    pub has_customer: bool,
    pub provider: Option<String>,
    pub note: String,
}

// ------------------------------------------------------------------ admin

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct AdminOverview {
    pub counts: SystemCounts,
    pub jobs: Vec<QueueStats>,
    pub pool: app_db::PoolStats,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct AdminUserInfo {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub status: String,
    pub system_role: String,
    pub email_verified: bool,
    pub identity_provider: String,
    #[ts(type = "string")]
    pub created_at: String,
    #[ts(type = "string | null")]
    pub last_login_at: Option<String>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct AdminUserDetail {
    pub user: AdminUserInfo,
    pub organizations: Vec<MyOrg>,
    #[ts(type = "number")]
    pub active_sessions: usize,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct RoleModel {
    pub key: String,
    pub permissions: Vec<String>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct PermissionDescription {
    pub key: String,
    pub description: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct RolesModelResponse {
    pub organization_roles: Vec<RoleModel>,
    pub system_roles: Vec<RoleModel>,
    pub permissions: Vec<PermissionDescription>,
    pub engine: String,
    pub system_roles_source: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct AdminJobs {
    pub stats: Vec<QueueStats>,
    pub items: Vec<JobRow>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ProviderInfo {
    pub name: String,
    pub base_url: String,
    #[ts(type = "number")]
    pub max_concurrency: usize,
    pub requests_per_second: f64,
    #[ts(type = "number")]
    pub tokens_per_minute: u64,
    /// Live state from the outbound engine when available.
    pub health: Option<ProviderHealth>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ProviderHealth {
    #[ts(type = "number")]
    pub concurrency_limit: usize,
    #[ts(type = "number")]
    pub inflight: usize,
    #[ts(type = "number")]
    pub queued: usize,
    pub circuit: String,
    pub success_rate: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub rate_429: f64,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ModulesInfo {
    pub cache: String,
    pub rate_limit: bool,
    pub messaging: bool,
    pub analytics: bool,
    pub organizations: bool,
    pub authorization_engine: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct IdentityInfo {
    pub provider: String,
    pub issuer: String,
    pub profile: String,
    pub require_mfa_for_system_admin: bool,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct SystemInfo {
    pub build: crate::state::BuildInfo,
    pub environment: String,
    pub checks: Vec<crate::health::CheckResult>,
    pub pool: Option<app_db::PoolStats>,
    pub modules: ModulesInfo,
    pub identity: IdentityInfo,
}

pub fn rfc3339(t: time::OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339).unwrap_or_default()
}

pub fn enum_str<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
}
