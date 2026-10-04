//! Permission vocabulary. Keys are stable strings (`resource:action`) used in the database,
//! API key scopes, Cedar policies and the frontend.

use serde::{Deserialize, Serialize};

macro_rules! permissions {
    ($name:ident { $($variant:ident => $key:literal, $desc:literal;)* }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(into = "&'static str", try_from = "String")]
        pub enum $name { $($variant),* }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),*];
            pub fn key(self) -> &'static str { match self { $($name::$variant => $key),* } }
            pub fn description(self) -> &'static str { match self { $($name::$variant => $desc),* } }
            pub fn parse(s: &str) -> Option<Self> { match s { $($key => Some($name::$variant),)* _ => None } }
        }

        impl From<$name> for &'static str { fn from(p: $name) -> Self { p.key() } }

        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(s: String) -> Result<Self, String> { Self::parse(&s).ok_or_else(|| format!("unknown permission {s}")) }
        }
    };
}

permissions!(Permission {
    OrgRead => "org:read", "View the organization";
    OrgUpdate => "org:update", "Rename the organization and change its profile";
    OrgDelete => "org:delete", "Delete the organization";
    OrgTransferOwnership => "org:transfer_ownership", "Grant or remove the owner role";
    MembersRead => "members:read", "List members";
    MembersInvite => "members:invite", "Invite new members";
    MembersRemove => "members:remove", "Remove members";
    MembersUpdateRole => "members:update_role", "Change members' roles";
    TeamsRead => "teams:read", "List teams";
    TeamsManage => "teams:manage", "Create, edit and delete teams";
    RolesRead => "roles:read", "List roles and their permissions";
    RolesManage => "roles:manage", "Create and edit custom roles";
    SettingsManage => "settings:manage", "Change organization settings";
    BillingRead => "billing:read", "View plan and billing state";
    BillingManage => "billing:manage", "Change plan and billing details";
    AuditRead => "audit:read", "Read the organization audit trail";
    ApiKeysRead => "api_keys:read", "List API keys";
    ApiKeysManage => "api_keys:manage", "Create, rotate and revoke API keys";
    RunsRead => "runs:read", "View runs";
    RunsCreate => "runs:create", "Create runs";
    RunsManageOwn => "runs:manage_own", "Cancel or delete runs you created";
    RunsManageAny => "runs:manage_any", "Cancel or delete any run";
});

permissions!(SystemPermission {
    UsersRead => "system:users:read", "List and view all users";
    UsersManage => "system:users:manage", "Suspend, reactivate and change system roles";
    OrgsRead => "system:orgs:read", "List and view all organizations";
    OrgsManage => "system:orgs:manage", "Suspend or delete organizations";
    AuditRead => "system:audit:read", "Read the global audit trail";
    JobsRead => "system:jobs:read", "View background jobs";
    JobsManage => "system:jobs:manage", "Retry or discard background jobs";
    ProvidersRead => "system:providers:read", "View external provider health";
    SystemRead => "system:health:read", "View system health and configuration summary";
    RolesRead => "system:roles:read", "View the role and permission model";
});

impl Permission {
    /// Owner-only powers: never grantable through custom roles, API keys or services.
    pub fn owner_only() -> PermissionSet {
        PermissionSet::from_iter([Permission::OrgDelete, Permission::OrgTransferOwnership, Permission::BillingManage])
    }

    pub fn assignable_to_custom_roles() -> PermissionSet {
        PermissionSet::all().minus(&Self::owner_only())
    }

    /// Permissions an API key or service client may carry (no membership administration).
    pub fn assignable_to_credentials() -> PermissionSet {
        Self::assignable_to_custom_roles().minus(&PermissionSet::from_iter([
            Permission::MembersInvite,
            Permission::MembersRemove,
            Permission::MembersUpdateRole,
            Permission::RolesManage,
            Permission::ApiKeysManage,
        ]))
    }
}

/// Small bitset over [`Permission`] (fits in a u64; asserted at compile time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PermissionSet(u64);

const _: () = assert!(Permission::ALL.len() <= 64);

impl PermissionSet {
    pub const fn empty() -> Self {
        Self(0)
    }
    pub fn all() -> Self {
        Self::from_iter(Permission::ALL.iter().copied())
    }
    fn bit(p: Permission) -> u64 {
        1u64 << (p as u8)
    }
    pub fn contains(&self, p: Permission) -> bool {
        self.0 & Self::bit(p) != 0
    }
    pub fn insert(&mut self, p: Permission) {
        self.0 |= Self::bit(p);
    }
    pub fn union(&self, o: &Self) -> Self {
        Self(self.0 | o.0)
    }
    pub fn intersect(&self, o: &Self) -> Self {
        Self(self.0 & o.0)
    }
    pub fn minus(&self, o: &Self) -> Self {
        Self(self.0 & !o.0)
    }
    pub fn is_subset_of(&self, o: &Self) -> bool {
        self.0 & !o.0 == 0
    }
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
    pub fn iter(&self) -> impl Iterator<Item = Permission> + '_ {
        Permission::ALL.iter().copied().filter(|p| self.contains(*p))
    }
    pub fn keys(&self) -> Vec<&'static str> {
        self.iter().map(Permission::key).collect()
    }
    /// Parse scope strings; unknown keys are an error (never silently ignored).
    pub fn parse_keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> Result<Self, String> {
        let mut s = Self::empty();
        for k in keys {
            s.insert(Permission::parse(k).ok_or_else(|| format!("unknown permission {k}"))?);
        }
        Ok(s)
    }
}

impl FromIterator<Permission> for PermissionSet {
    fn from_iter<I: IntoIterator<Item = Permission>>(iter: I) -> Self {
        let mut s = Self::empty();
        for p in iter {
            s.insert(p);
        }
        s
    }
}

impl Serialize for PermissionSet {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.iter().map(Permission::key))
    }
}
