//! Authorization. Authentication (who) lives in `app-auth`; this crate decides what an
//! already-authenticated actor may do. Invariants (see .ai/knowledge/CONSTRAINTS.md):
//!
//! - **Deny by default.** Every decision starts at deny; only an explicit rule allows.
//! - **The backend is the boundary.** The frontend may *hide* things using
//!   [`OrgAccess::permissions`], but every request is re-checked here.
//! - **Tenant isolation.** Organisation data is reachable only through an [`OrgAccess`],
//!   which can only be obtained from an [`Authorizer`] given server-verified membership facts.
//!   Repository functions take `&OrgAccess`, so an unscoped query does not compile.
//! - **System vs organisation trust.** [`SystemRole`] grants [`SystemPermission`]s only;
//!   organisation roles never imply system access and vice versa.
//!
//! Engines: [`rbac::Rbac`] (default) and, with feature `cedar`, `cedar::CedarAuthorizer`.
//! Both must pass the shared conformance suite in `tests/conformance.rs`.

pub mod permission;
pub mod rbac;
#[cfg(feature = "cedar")]
pub mod cedar;

use std::time::Duration;

pub use app_domain::{OrgRole, SystemRole};
pub use permission::{Permission, PermissionSet, SystemPermission};
use uuid::Uuid;

/// The authenticated caller, as established by `app-auth`. Constructed only from verified
/// credentials (session, JWT, API key); never from request parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    User(UserActor),
    /// OAuth client / service account registered to one organisation. Never a user.
    Service { id: Uuid, organization_id: Uuid, scopes: PermissionSet },
    /// API key owned by one organisation; effective permissions are its scopes intersected
    /// with its creator's *current* permissions.
    ApiKey { id: Uuid, organization_id: Uuid, scopes: PermissionSet, created_by: Option<Uuid> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserActor {
    pub id: Uuid,
    pub system_role: SystemRole,
    pub active: bool,
    /// Authentication context of the current session.
    pub mfa: bool,
    pub auth_age: Duration,
}

impl Actor {
    pub fn user_id(&self) -> Option<Uuid> {
        match self {
            Actor::User(u) => Some(u.id),
            _ => None,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Actor::User(_) => "user",
            Actor::Service { .. } => "service",
            Actor::ApiKey { .. } => "api_key",
        }
    }
    pub fn id(&self) -> Uuid {
        match self {
            Actor::User(u) => u.id,
            Actor::Service { id, .. } | Actor::ApiKey { id, .. } => *id,
        }
    }
}

/// Server-verified facts about an actor's relationship to one organisation, loaded from the
/// database by the caller. `role` is `None` when the actor is not a member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipFacts {
    pub organization_id: Uuid,
    pub organization_deleted: bool,
    pub role: Option<RoleGrant>,
    /// For API keys: the creator's current grant in this organisation (None if they left).
    pub creator_role: Option<RoleGrant>,
}

/// A role and the permissions it grants. Built-in roles carry `builtin = Some(role)` and
/// their permissions come from code; custom roles carry their stored permission set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleGrant {
    pub role_id: Uuid,
    pub key: String,
    pub builtin: Option<OrgRole>,
    pub custom_permissions: PermissionSet,
}

impl RoleGrant {
    pub fn builtin(role: OrgRole) -> Self {
        Self {
            role_id: rbac::builtin_role_id(role),
            key: role.as_str().to_string(),
            builtin: Some(role),
            custom_permissions: PermissionSet::empty(),
        }
    }

    /// Effective permissions. Custom roles can never hold owner-only permissions.
    pub fn permissions(&self) -> PermissionSet {
        match self.builtin {
            Some(r) => rbac::builtin_permissions(r),
            None => self.custom_permissions.intersect(&Permission::assignable_to_custom_roles()),
        }
    }
}

/// Proof that an actor was authorized into one organisation, with the permissions it holds
/// there. Repositories accept only this (never a raw organisation id) for tenant data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgAccess {
    org_id: Uuid,
    actor: Actor,
    role: Option<RoleGrant>,
    permissions: PermissionSet,
}

impl OrgAccess {
    pub fn org_id(&self) -> Uuid {
        self.org_id
    }
    pub fn actor(&self) -> &Actor {
        &self.actor
    }
    pub fn role(&self) -> Option<&RoleGrant> {
        self.role.as_ref()
    }
    pub fn permissions(&self) -> &PermissionSet {
        &self.permissions
    }
    pub fn can(&self, p: Permission) -> bool {
        self.permissions.contains(p)
    }
    pub fn require(&self, p: Permission) -> Result<(), Denied> {
        if self.can(p) { Ok(()) } else { Err(Denied::MissingPermission(p.key())) }
    }

    /// Test-only constructor for repository tests that need a scope without an engine.
    #[doc(hidden)]
    pub fn __for_tests(org_id: Uuid, actor: Actor, permissions: PermissionSet) -> Self {
        Self { org_id, actor, role: None, permissions }
    }
}

/// Why access was denied. Mapped to 403 (or 404 where existence must not leak) by the API.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Denied {
    #[error("not a member of this organization")]
    NotMember,
    #[error("missing permission {0}")]
    MissingPermission(&'static str),
    #[error("system privilege required")]
    SystemPrivilegeRequired,
    #[error("multi-factor authentication required")]
    MfaRequired,
    #[error("recent authentication required")]
    ReauthenticationRequired,
    #[error("account is not active")]
    InactiveAccount,
    #[error("would grant privileges the actor does not hold")]
    Escalation,
    #[error("an organization must keep at least one owner")]
    LastOwner,
    #[error("operation not permitted: {0}")]
    Forbidden(&'static str),
}

/// What is being acted on. Carries only the attributes policies need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    Organization,
    /// A resource with an owner inside the organisation (e.g. a run).
    Owned { owner_id: Option<Uuid> },
    /// A membership: the target user and their current role.
    Member { user_id: Uuid, role: RoleGrant },
}

/// Request-level context for conditional policies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    pub require_mfa_for_system: bool,
    /// Maximum authentication age for sensitive actions (None = not required).
    pub max_auth_age: Option<Duration>,
}

pub trait Authorizer: Send + Sync {
    /// Resolve organisation access. Deny if the actor is not a member (or the key/service
    /// belongs to another organisation), the organisation is deleted, or the user is inactive.
    fn org_access(&self, actor: &Actor, facts: &MembershipFacts) -> Result<OrgAccess, Denied>;

    /// Decide an action on a resource inside an organisation the actor already has access to.
    fn authorize(&self, access: &OrgAccess, action: Permission, resource: &Resource) -> Result<(), Denied>;

    /// Decide a system-level action (system admin APIs).
    fn authorize_system(&self, actor: &Actor, action: SystemPermission, ctx: &Context) -> Result<(), Denied>;

    /// Decide whether `access` may give `target` the role `new_role` (or remove it when None).
    fn authorize_role_change(
        &self,
        access: &OrgAccess,
        target_user: Uuid,
        current: &RoleGrant,
        new_role: Option<&RoleGrant>,
        owner_count: usize,
    ) -> Result<(), Denied>;
}

pub(crate) fn new_access(org_id: Uuid, actor: Actor, role: Option<RoleGrant>, permissions: PermissionSet) -> OrgAccess {
    OrgAccess { org_id, actor, role, permissions }
}
