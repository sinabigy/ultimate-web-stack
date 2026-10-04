#![allow(clippy::unwrap_used)]
//! Authorization conformance suite. Every engine must produce exactly these decisions.
//! Covers: tenant isolation, missing permissions, ownership, escalation, last owner,
//! system vs organisation trust, MFA, credentials.

use std::time::Duration;

use app_authz::{
    Actor, Authorizer, Context, Denied, MembershipFacts, OrgRole, Permission as P, PermissionSet, Resource, RoleGrant,
    SystemPermission as S, SystemRole, UserActor, rbac::Rbac,
};
use uuid::Uuid;

fn engines() -> Vec<(&'static str, Box<dyn Authorizer>)> {
    #[allow(unused_mut)]
    let mut v: Vec<(&'static str, Box<dyn Authorizer>)> = vec![("rbac", Box::new(Rbac))];
    #[cfg(feature = "cedar")]
    v.push(("cedar", Box::new(app_authz::cedar::CedarAuthorizer::new_default().unwrap())));
    v
}

fn user(system_role: SystemRole) -> Actor {
    Actor::User(UserActor { id: Uuid::now_v7(), system_role, active: true, mfa: false, auth_age: Duration::ZERO })
}

fn facts(org: Uuid, role: Option<OrgRole>) -> MembershipFacts {
    MembershipFacts {
        organization_id: org,
        organization_deleted: false,
        role: role.map(RoleGrant::builtin),
        creator_role: None,
    }
}

#[test]
fn tenant_a_user_cannot_access_tenant_b() {
    for (name, e) in engines() {
        let (org_a, org_b) = (Uuid::now_v7(), Uuid::now_v7());
        let alice = user(SystemRole::None);
        assert!(e.org_access(&alice, &facts(org_a, Some(OrgRole::Owner))).is_ok(), "{name}");
        assert_eq!(e.org_access(&alice, &facts(org_b, None)).unwrap_err(), Denied::NotMember, "{name}");
    }
}

#[test]
fn system_admin_gets_no_implicit_org_access() {
    for (name, e) in engines() {
        let root = user(SystemRole::SystemAdmin);
        assert_eq!(e.org_access(&root, &facts(Uuid::now_v7(), None)).unwrap_err(), Denied::NotMember, "{name}");
    }
}

#[test]
fn org_admin_never_gains_system_access() {
    for (name, e) in engines() {
        let admin = user(SystemRole::None);
        let ctx = Context::default();
        for p in S::ALL {
            assert_eq!(
                e.authorize_system(&admin, *p, &ctx).unwrap_err(),
                Denied::SystemPrivilegeRequired,
                "{name} {p:?}"
            );
        }
    }
}

#[test]
fn deleted_org_and_inactive_user_denied() {
    for (name, e) in engines() {
        let org = Uuid::now_v7();
        let mut f = facts(org, Some(OrgRole::Owner));
        f.organization_deleted = true;
        assert!(e.org_access(&user(SystemRole::None), &f).is_err(), "{name}");
        let inactive = Actor::User(UserActor {
            id: Uuid::now_v7(),
            system_role: SystemRole::None,
            active: false,
            mfa: true,
            auth_age: Duration::ZERO,
        });
        assert_eq!(
            e.org_access(&inactive, &facts(org, Some(OrgRole::Owner))).unwrap_err(),
            Denied::InactiveAccount,
            "{name}"
        );
    }
}

#[test]
fn role_permission_matrix() {
    let expect: &[(OrgRole, P, bool)] = &[
        (OrgRole::Viewer, P::RunsRead, true),
        (OrgRole::Viewer, P::RunsCreate, false),
        (OrgRole::Member, P::RunsCreate, true),
        (OrgRole::Member, P::MembersInvite, false),
        (OrgRole::Manager, P::MembersInvite, true),
        (OrgRole::Manager, P::MembersRemove, false),
        (OrgRole::Admin, P::MembersRemove, true),
        (OrgRole::Admin, P::AuditRead, true),
        (OrgRole::Admin, P::OrgDelete, false),
        (OrgRole::Admin, P::BillingManage, false),
        (OrgRole::Owner, P::OrgDelete, true),
    ];
    for (name, e) in engines() {
        let org = Uuid::now_v7();
        for (role, perm, allowed) in expect {
            let access = e.org_access(&user(SystemRole::None), &facts(org, Some(*role))).unwrap();
            assert_eq!(
                e.authorize(&access, *perm, &Resource::Organization).is_ok(),
                *allowed,
                "{name}: {role:?} {perm:?}"
            );
        }
    }
}

#[test]
fn ownership_rules_for_owned_resources() {
    for (name, e) in engines() {
        let org = Uuid::now_v7();
        let member = user(SystemRole::None);
        let access = e.org_access(&member, &facts(org, Some(OrgRole::Member))).unwrap();
        let mine = Resource::Owned { owner_id: member.user_id() };
        let theirs = Resource::Owned { owner_id: Some(Uuid::now_v7()) };
        assert!(e.authorize(&access, P::RunsManageOwn, &mine).is_ok(), "{name}");
        assert!(e.authorize(&access, P::RunsManageOwn, &theirs).is_err(), "{name}");
        assert!(e.authorize(&access, P::RunsManageOwn, &Resource::Owned { owner_id: None }).is_err(), "{name}");
        let manager = e.org_access(&user(SystemRole::None), &facts(org, Some(OrgRole::Manager))).unwrap();
        assert!(e.authorize(&manager, P::RunsManageAny, &theirs).is_ok(), "{name}");
    }
}

#[test]
fn no_self_escalation_and_no_granting_more_than_you_hold() {
    for (name, e) in engines() {
        let org = Uuid::now_v7();
        let admin = user(SystemRole::None);
        let access = e.org_access(&admin, &facts(org, Some(OrgRole::Admin))).unwrap();
        let me = admin.user_id().unwrap();
        let member = RoleGrant::builtin(OrgRole::Member);
        let admin_r = RoleGrant::builtin(OrgRole::Admin);
        let owner_r = RoleGrant::builtin(OrgRole::Owner);
        // self role change
        assert!(e.authorize_role_change(&access, me, &admin_r, Some(&owner_r), 1).is_err(), "{name}");
        // admin promoting someone to owner
        let other = Uuid::now_v7();
        assert!(e.authorize_role_change(&access, other, &member, Some(&owner_r), 1).is_err(), "{name}");
        // admin demoting an owner
        assert!(e.authorize_role_change(&access, other, &owner_r, Some(&member), 2).is_err(), "{name}");
        // admin promoting member to admin is fine
        assert!(e.authorize_role_change(&access, other, &member, Some(&admin_r), 1).is_ok(), "{name}");
        // manager cannot change roles at all
        let mgr = e.org_access(&user(SystemRole::None), &facts(org, Some(OrgRole::Manager))).unwrap();
        assert!(
            e.authorize_role_change(&mgr, other, &member, Some(&RoleGrant::builtin(OrgRole::Viewer)), 1).is_err(),
            "{name}"
        );
    }
}

#[test]
fn custom_roles_cannot_carry_owner_powers() {
    for (name, e) in engines() {
        let org = Uuid::now_v7();
        let custom = RoleGrant {
            role_id: Uuid::now_v7(),
            key: "superuser".into(),
            builtin: None,
            custom_permissions: PermissionSet::all(),
        };
        let f = MembershipFacts {
            organization_id: org,
            organization_deleted: false,
            role: Some(custom),
            creator_role: None,
        };
        let access = e.org_access(&user(SystemRole::None), &f).unwrap();
        assert!(
            !access.can(P::OrgDelete) && !access.can(P::OrgTransferOwnership) && !access.can(P::BillingManage),
            "{name}"
        );
        assert!(access.can(P::MembersRemove), "{name}");
    }
}

#[test]
fn last_owner_is_protected() {
    for (name, e) in engines() {
        let org = Uuid::now_v7();
        let owner = user(SystemRole::None);
        let access = e.org_access(&owner, &facts(org, Some(OrgRole::Owner))).unwrap();
        let owner_r = RoleGrant::builtin(OrgRole::Owner);
        let me = owner.user_id().unwrap();
        assert_eq!(e.authorize_role_change(&access, me, &owner_r, None, 1).unwrap_err(), Denied::LastOwner, "{name}");
        assert!(
            e.authorize_role_change(&access, me, &owner_r, None, 2).is_ok(),
            "{name}: may leave if another owner exists"
        );
        let other = Uuid::now_v7();
        assert_eq!(
            e.authorize_role_change(&access, other, &owner_r, Some(&RoleGrant::builtin(OrgRole::Admin)), 1)
                .unwrap_err(),
            Denied::LastOwner,
            "{name}"
        );
    }
}

#[test]
fn system_roles_and_mfa() {
    for (name, e) in engines() {
        let auditor = user(SystemRole::SystemAuditor);
        let ctx = Context::default();
        assert!(e.authorize_system(&auditor, S::AuditRead, &ctx).is_ok(), "{name}");
        assert!(e.authorize_system(&auditor, S::UsersManage, &ctx).is_err(), "{name}");
        let admin = user(SystemRole::SystemAdmin);
        let strict = Context { require_mfa_for_system: true, max_auth_age: None };
        assert_eq!(e.authorize_system(&admin, S::UsersManage, &strict).unwrap_err(), Denied::MfaRequired, "{name}");
        let mfa_admin = Actor::User(UserActor {
            id: Uuid::now_v7(),
            system_role: SystemRole::SystemAdmin,
            active: true,
            mfa: true,
            auth_age: Duration::from_secs(3600),
        });
        assert!(e.authorize_system(&mfa_admin, S::UsersManage, &strict).is_ok(), "{name}");
        let recent = Context { require_mfa_for_system: true, max_auth_age: Some(Duration::from_secs(600)) };
        assert_eq!(
            e.authorize_system(&mfa_admin, S::UsersManage, &recent).unwrap_err(),
            Denied::ReauthenticationRequired,
            "{name}"
        );
    }
}

#[test]
fn api_keys_bounded_by_scopes_creator_and_org() {
    for (name, e) in engines() {
        let (org, other_org) = (Uuid::now_v7(), Uuid::now_v7());
        let scopes = PermissionSet::from_iter([P::RunsRead, P::RunsCreate, P::MembersRemove]);
        let key = Actor::ApiKey { id: Uuid::now_v7(), organization_id: org, scopes, created_by: Some(Uuid::now_v7()) };
        let mut f = facts(org, None);
        f.creator_role = Some(RoleGrant::builtin(OrgRole::Viewer));
        let access = e.org_access(&key, &f).unwrap();
        assert!(access.can(P::RunsRead), "{name}");
        assert!(!access.can(P::RunsCreate), "{name}: creator (viewer) cannot create");
        assert!(!access.can(P::MembersRemove), "{name}: never credential-assignable");
        f.creator_role = None;
        assert!(e.org_access(&key, &f).is_err(), "{name}: creator left the org");
        let mut g = facts(other_org, None);
        g.creator_role = Some(RoleGrant::builtin(OrgRole::Owner));
        assert!(e.org_access(&key, &g).is_err(), "{name}: key bound to its own org");
        assert!(e.authorize_system(&key, S::SystemRead, &Context::default()).is_err(), "{name}");
    }
}

#[test]
fn acting_on_more_privileged_members_is_escalation() {
    for (name, e) in engines() {
        let org = Uuid::now_v7();
        let admin = e.org_access(&user(SystemRole::None), &facts(org, Some(OrgRole::Admin))).unwrap();
        let owner_member = Resource::Member { user_id: Uuid::now_v7(), role: RoleGrant::builtin(OrgRole::Owner) };
        assert!(e.authorize(&admin, P::MembersRemove, &owner_member).is_err(), "{name}");
        let member = Resource::Member { user_id: Uuid::now_v7(), role: RoleGrant::builtin(OrgRole::Member) };
        assert!(e.authorize(&admin, P::MembersRemove, &member).is_ok(), "{name}");
    }
}

#[test]
fn builtin_roles_are_strictly_nested() {
    use app_authz::rbac::builtin_permissions as bp;
    let order = [OrgRole::Viewer, OrgRole::Member, OrgRole::Manager, OrgRole::Admin, OrgRole::Owner];
    for w in order.windows(2) {
        assert!(bp(w[0]).is_subset_of(&bp(w[1])) && bp(w[0]) != bp(w[1]), "{:?} ⊂ {:?}", w[0], w[1]);
    }
}

#[cfg(feature = "cedar")]
mod differential {
    use super::*;
    use app_authz::cedar::CedarAuthorizer;
    use proptest::prelude::*;

    fn role_strategy() -> impl Strategy<Value = OrgRole> {
        prop::sample::select(OrgRole::ALL.to_vec())
    }
    fn perm_strategy() -> impl Strategy<Value = P> {
        prop::sample::select(P::ALL.to_vec())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(400))]
        /// RBAC and Cedar must agree on every decision (allow/deny and reason).
        #[test]
        fn engines_agree(actor_role in role_strategy(), target_role in role_strategy(), new_role in proptest::option::of(role_strategy()),
                         action in perm_strategy(), own in any::<bool>(), is_self in any::<bool>(), owners in 1usize..3,
                         sys in prop::sample::select(vec![SystemRole::None, SystemRole::SystemAuditor, SystemRole::SystemAdmin]),
                         sys_perm in prop::sample::select(S::ALL.to_vec()), mfa in any::<bool>(), need_mfa in any::<bool>()) {
            let cedar = CedarAuthorizer::new_default().unwrap();
            let org = Uuid::now_v7();
            let me = Uuid::now_v7();
            let actor = Actor::User(UserActor { id: me, system_role: sys, active: true, mfa, auth_age: Duration::from_secs(30) });
            let f = facts(org, Some(actor_role));
            let a = Rbac.org_access(&actor, &f).unwrap();
            let c = cedar.org_access(&actor, &f).unwrap();
            prop_assert_eq!(a.permissions(), c.permissions());

            let owned = Resource::Owned { owner_id: Some(if own { me } else { Uuid::now_v7() }) };
            prop_assert_eq!(Rbac.authorize(&a, action, &owned), cedar.authorize(&c, action, &owned));
            prop_assert_eq!(Rbac.authorize(&a, action, &Resource::Organization), cedar.authorize(&c, action, &Resource::Organization));
            let member = Resource::Member { user_id: Uuid::now_v7(), role: RoleGrant::builtin(target_role) };
            prop_assert_eq!(Rbac.authorize(&a, action, &member), cedar.authorize(&c, action, &member));

            let target = if is_self { me } else { Uuid::now_v7() };
            let cur = RoleGrant::builtin(target_role);
            let new = new_role.map(RoleGrant::builtin);
            prop_assert_eq!(
                Rbac.authorize_role_change(&a, target, &cur, new.as_ref(), owners),
                cedar.authorize_role_change(&c, target, &cur, new.as_ref(), owners)
            );

            let ctx = Context { require_mfa_for_system: need_mfa, max_auth_age: None };
            prop_assert_eq!(Rbac.authorize_system(&actor, sys_perm, &ctx), cedar.authorize_system(&actor, sys_perm, &ctx));
        }
    }
}
