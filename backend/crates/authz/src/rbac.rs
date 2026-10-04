//! Default engine: role → permissions, plus ownership, escalation and last-owner rules.

use uuid::Uuid;

use crate::{
    Actor, Authorizer, Context, Denied, MembershipFacts, OrgAccess, OrgRole, Permission as P, PermissionSet, Resource,
    RoleGrant, SystemPermission as S, SystemRole, new_access,
};

/// Fixed ids of built-in roles (must match migration 20261004000003_tenancy.sql).
pub fn builtin_role_id(role: OrgRole) -> Uuid {
    let n: u128 = match role {
        OrgRole::Owner => 1,
        OrgRole::Admin => 2,
        OrgRole::Manager => 3,
        OrgRole::Member => 4,
        OrgRole::Viewer => 5,
    };
    Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0000 | n)
}

pub fn builtin_role_for_id(id: Uuid) -> Option<OrgRole> {
    OrgRole::ALL.into_iter().find(|r| builtin_role_id(*r) == id)
}

/// Built-in role permissions. Each role is a strict superset of the one below it.
pub fn builtin_permissions(role: OrgRole) -> PermissionSet {
    let viewer = PermissionSet::from_iter([P::OrgRead, P::MembersRead, P::TeamsRead, P::RolesRead, P::RunsRead]);
    let member = viewer.union(&PermissionSet::from_iter([P::RunsCreate, P::RunsManageOwn]));
    let manager =
        member.union(&PermissionSet::from_iter([P::MembersInvite, P::TeamsManage, P::RunsManageAny, P::ApiKeysRead]));
    let admin = manager.union(&PermissionSet::from_iter([
        P::MembersRemove,
        P::MembersUpdateRole,
        P::RolesManage,
        P::SettingsManage,
        P::OrgUpdate,
        P::AuditRead,
        P::ApiKeysManage,
        P::BillingRead,
    ]));
    match role {
        OrgRole::Viewer => viewer,
        OrgRole::Member => member,
        OrgRole::Manager => manager,
        OrgRole::Admin => admin,
        OrgRole::Owner => PermissionSet::all(),
    }
}

pub fn system_permissions(role: SystemRole) -> &'static [S] {
    match role {
        SystemRole::None => &[],
        SystemRole::SystemAuditor => {
            &[S::UsersRead, S::OrgsRead, S::AuditRead, S::JobsRead, S::ProvidersRead, S::SystemRead, S::RolesRead]
        }
        SystemRole::SystemAdmin => S::ALL,
    }
}

fn is_owner(r: &RoleGrant) -> bool {
    r.builtin == Some(OrgRole::Owner)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Rbac;

impl Authorizer for Rbac {
    fn org_access(&self, actor: &Actor, facts: &MembershipFacts) -> Result<OrgAccess, Denied> {
        if facts.organization_deleted {
            return Err(Denied::NotMember);
        }
        match actor {
            Actor::User(u) => {
                if !u.active {
                    return Err(Denied::InactiveAccount);
                }
                // System roles grant NO organisation access: support uses /admin APIs.
                let role = facts.role.clone().ok_or(Denied::NotMember)?;
                let perms = role.permissions();
                Ok(new_access(facts.organization_id, actor.clone(), Some(role), perms))
            }
            Actor::Service { organization_id, scopes, .. } => {
                if *organization_id != facts.organization_id {
                    return Err(Denied::NotMember);
                }
                let perms = scopes.intersect(&P::assignable_to_credentials());
                Ok(new_access(facts.organization_id, actor.clone(), None, perms))
            }
            Actor::ApiKey { organization_id, scopes, .. } => {
                if *organization_id != facts.organization_id {
                    return Err(Denied::NotMember);
                }
                // A key can never exceed what its creator can currently do.
                let creator = facts.creator_role.as_ref().ok_or(Denied::NotMember)?;
                let perms = scopes.intersect(&creator.permissions()).intersect(&P::assignable_to_credentials());
                Ok(new_access(facts.organization_id, actor.clone(), None, perms))
            }
        }
    }

    fn authorize(&self, access: &OrgAccess, action: P, resource: &Resource) -> Result<(), Denied> {
        match resource {
            Resource::Owned { owner_id } if matches!(action, P::RunsManageOwn | P::RunsManageAny) => {
                if access.can(P::RunsManageAny) {
                    return Ok(());
                }
                let mine = owner_id.is_some() && *owner_id == access.actor().user_id();
                if mine && access.can(P::RunsManageOwn) {
                    Ok(())
                } else {
                    Err(Denied::MissingPermission(P::RunsManageAny.key()))
                }
            }
            Resource::Member { role, .. } => {
                access.require(action)?;
                // Nobody acts on a member more privileged than themselves.
                if !role.permissions().is_subset_of(access.permissions()) {
                    return Err(Denied::Escalation);
                }
                if is_owner(role) && !access.can(P::OrgTransferOwnership) {
                    return Err(Denied::MissingPermission(P::OrgTransferOwnership.key()));
                }
                Ok(())
            }
            _ => access.require(action),
        }
    }

    fn authorize_system(&self, actor: &Actor, action: S, ctx: &Context) -> Result<(), Denied> {
        let Actor::User(u) = actor else { return Err(Denied::SystemPrivilegeRequired) };
        if !u.active {
            return Err(Denied::InactiveAccount);
        }
        if !system_permissions(u.system_role).contains(&action) {
            return Err(Denied::SystemPrivilegeRequired);
        }
        if ctx.require_mfa_for_system && !u.mfa {
            return Err(Denied::MfaRequired);
        }
        if let Some(max) = ctx.max_auth_age
            && u.auth_age > max
        {
            return Err(Denied::ReauthenticationRequired);
        }
        Ok(())
    }

    fn authorize_role_change(
        &self,
        access: &OrgAccess,
        target_user: Uuid,
        current: &RoleGrant,
        new_role: Option<&RoleGrant>,
        owner_count: usize,
    ) -> Result<(), Denied> {
        let is_self = access.actor().user_id() == Some(target_user);
        let losing_owner = is_owner(current) && new_role.is_none_or(|r| !is_owner(r));
        if losing_owner && owner_count <= 1 {
            return Err(Denied::LastOwner);
        }
        if is_self {
            // Members may leave; nobody may change their own role (no self-escalation,
            // and self-demotion of owners goes through ownership transfer).
            return if new_role.is_none() { Ok(()) } else { Err(Denied::Forbidden("cannot change your own role")) };
        }
        let needed = if new_role.is_some() { P::MembersUpdateRole } else { P::MembersRemove };
        access.require(needed)?;
        let touches_owner = is_owner(current) || new_role.is_some_and(is_owner);
        if touches_owner && !access.can(P::OrgTransferOwnership) {
            return Err(Denied::MissingPermission(P::OrgTransferOwnership.key()));
        }
        if !current.permissions().is_subset_of(access.permissions()) {
            return Err(Denied::Escalation);
        }
        if let Some(r) = new_role
            && !r.permissions().is_subset_of(access.permissions())
        {
            return Err(Denied::Escalation);
        }
        Ok(())
    }
}

impl Rbac {
    /// Inviting with a role is granting it: same escalation rules as a role change.
    pub fn authorize_invite(&self, access: &OrgAccess, role: &RoleGrant) -> Result<(), Denied> {
        access.require(P::MembersInvite)?;
        if is_owner(role) && !access.can(P::OrgTransferOwnership) {
            return Err(Denied::MissingPermission(P::OrgTransferOwnership.key()));
        }
        if !role.permissions().is_subset_of(access.permissions()) {
            return Err(Denied::Escalation);
        }
        Ok(())
    }

    /// Creating credentials (API keys, service clients): scopes must be credential-assignable
    /// and held by the creator.
    pub fn authorize_credential_scopes(&self, access: &OrgAccess, scopes: &PermissionSet) -> Result<(), Denied> {
        access.require(P::ApiKeysManage)?;
        if scopes.is_empty() || !scopes.is_subset_of(&P::assignable_to_credentials()) {
            return Err(Denied::Forbidden("scopes not assignable to credentials"));
        }
        if !scopes.is_subset_of(access.permissions()) {
            return Err(Denied::Escalation);
        }
        Ok(())
    }
}
