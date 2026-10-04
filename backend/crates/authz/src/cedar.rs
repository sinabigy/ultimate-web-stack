//! Cedar authorization engine (feature `cedar`).
//!
//! Responsibilities are split deliberately:
//! - **Rust resolves facts**: who the actor is, whether they are an active member, which
//!   permissions their role or credential scopes grant (same data as the RBAC engine).
//! - **Cedar decides**: every decision is a Cedar evaluation over those facts, using
//!   `policies/base.cedar` plus any project policies (`*.cedar` in the configured directory).
//!   Project policies can add `forbid` rules or ABAC `permit` rules without touching Rust.
//!
//! Denial reasons come from `@reason("...")` annotations on the deciding `forbid` policy.

use std::{path::Path, str::FromStr};

use cedar_policy::{
    Authorizer as CedarAuth, Context as CedarContext, Decision, Entities, EntityUid, PolicySet, Request,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    Actor, Authorizer, Context, Denied, MembershipFacts, OrgAccess, Permission, PermissionSet, Resource, RoleGrant,
    SystemPermission, new_access, rbac,
};

pub const BASE_POLICIES: &str = include_str!("../policies/base.cedar");

pub struct CedarAuthorizer {
    policies: PolicySet,
    engine: CedarAuth,
}

#[derive(Debug, thiserror::Error)]
pub enum CedarSetupError {
    #[error("policy parse error in {source_name}: {message}")]
    Parse { source_name: String, message: String },
    #[error("reading policies: {0}")]
    Io(#[from] std::io::Error),
}

impl CedarAuthorizer {
    pub fn new_default() -> Result<Self, CedarSetupError> {
        Self::from_sources(&[("base.cedar".into(), BASE_POLICIES.into())])
    }

    /// Base policies plus every `*.cedar` file in `dir` (sorted, for determinism).
    pub fn with_policy_dir(dir: &Path) -> Result<Self, CedarSetupError> {
        let mut sources = vec![("base.cedar".to_string(), BASE_POLICIES.to_string())];
        if dir.is_dir() {
            let mut files: Vec<_> = std::fs::read_dir(dir)?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "cedar"))
                .collect();
            files.sort();
            for f in files {
                sources.push((f.display().to_string(), std::fs::read_to_string(&f)?));
            }
        }
        Self::from_sources(&sources)
    }

    fn from_sources(sources: &[(String, String)]) -> Result<Self, CedarSetupError> {
        // Parse each source separately for precise errors, then combine (ids are re-assigned
        // per source by Cedar, so concatenate the text to keep ids unique).
        for (name, text) in sources {
            PolicySet::from_str(text)
                .map_err(|e| CedarSetupError::Parse { source_name: name.clone(), message: e.to_string() })?;
        }
        let all = sources.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join("\n");
        let policies = PolicySet::from_str(&all)
            .map_err(|e| CedarSetupError::Parse { source_name: "combined".into(), message: e.to_string() })?;
        Ok(Self { policies, engine: CedarAuth::new() })
    }

    fn uid(ty: &str, id: &str) -> EntityUid {
        // Ids are UUIDs or fixed keys: no quotes/backslashes, so this cannot inject.
        EntityUid::from_str(&format!("{ty}::\"{id}\"")).unwrap_or_else(|_| unreachable!("valid entity uid"))
    }

    fn principal_json(actor: &Actor, permissions: &PermissionSet) -> Value {
        let (kind, user_id, active, system_role, mfa, age) = match actor {
            Actor::User(u) => ("user", u.id.to_string(), u.active, u.system_role.as_str(), u.mfa, u.auth_age.as_secs()),
            Actor::Service { .. } => ("service", String::new(), true, "none", false, 0),
            Actor::ApiKey { .. } => ("api_key", String::new(), true, "none", false, 0),
        };
        json!({
            "uid": {"type": "Principal", "id": actor.id().to_string()},
            "attrs": {
                "kind": kind, "user_id": user_id, "active": active, "system_role": system_role,
                "mfa": mfa, "auth_age_secs": i64::try_from(age).unwrap_or(i64::MAX),
                "permissions": permissions.keys(),
            },
            "parents": []
        })
    }

    /// Evaluate; returns Ok or the set of `@reason` annotations of satisfied forbid policies.
    fn eval(
        &self,
        principal: Value,
        action: &str,
        resource: (&str, &str, Value),
        context: Value,
    ) -> Result<(), Vec<String>> {
        let entities_json = json!([
            principal.clone(),
            {"uid": {"type": resource.0, "id": resource.1}, "attrs": resource.2, "parents": []}
        ]);
        let entities = match Entities::from_json_value(entities_json, None) {
            Ok(e) => e,
            Err(e) => return Err(vec![format!("entity error: {e}")]), // fail closed
        };
        let pid = principal["uid"]["id"].as_str().unwrap_or_default().to_string();
        let ctx = match CedarContext::from_json_value(context, None) {
            Ok(c) => c,
            Err(e) => return Err(vec![format!("context error: {e}")]),
        };
        let req = match Request::new(
            Self::uid("Principal", &pid),
            Self::uid("Action", action),
            Self::uid(resource.0, resource.1),
            ctx,
            None,
        ) {
            Ok(r) => r,
            Err(e) => return Err(vec![format!("request error: {e}")]),
        };
        let resp = self.engine.is_authorized(&req, &self.policies, &entities);
        // Evaluation errors in any policy are treated as deny (never fail open).
        if resp.diagnostics().errors().next().is_some() {
            return Err(vec!["evaluation_error".into()]);
        }
        match resp.decision() {
            Decision::Allow => Ok(()),
            Decision::Deny => Err(resp
                .diagnostics()
                .reason()
                .filter_map(|id| self.policies.policy(id))
                .filter_map(|p| p.annotation("reason").map(str::to_string))
                .collect()),
        }
    }

    fn first_reason(reasons: &[String], order: &[(&str, Denied)]) -> Option<Denied> {
        order.iter().find(|(r, _)| reasons.iter().any(|x| x == r)).map(|(_, d)| d.clone())
    }
}

fn resource_json(resource: &Resource) -> (&'static str, String, Value) {
    match resource {
        Resource::Organization => ("Organization", "org".into(), json!({})),
        Resource::Owned { owner_id } => {
            ("Owned", "resource".into(), json!({"owner": owner_id.map(|u| u.to_string()).unwrap_or_default()}))
        }
        Resource::Member { user_id, role } => (
            "Member",
            user_id.to_string(),
            json!({"permissions": role.permissions().keys(), "is_owner": is_owner(role)}),
        ),
    }
}

fn is_owner(r: &RoleGrant) -> bool {
    r.builtin == Some(crate::OrgRole::Owner)
}

impl Authorizer for CedarAuthorizer {
    fn org_access(&self, actor: &Actor, facts: &MembershipFacts) -> Result<OrgAccess, Denied> {
        // Fact resolution is shared with RBAC; Cedar then filters every candidate permission,
        // so project policies can forbid specific permissions for specific principals.
        let base = rbac::Rbac.org_access(actor, facts)?;
        let candidates = *base.permissions();
        let principal = Self::principal_json(actor, &candidates);
        let allowed: PermissionSet = candidates
            .iter()
            .filter(|p| {
                self.eval(
                    principal.clone(),
                    p.key(),
                    ("Organization", "org", json!({})),
                    json!({"permission": p.key()}),
                )
                .is_ok()
            })
            .collect();
        Ok(new_access(facts.organization_id, actor.clone(), base.role().cloned(), allowed))
    }

    fn authorize(&self, access: &OrgAccess, action: Permission, resource: &Resource) -> Result<(), Denied> {
        let principal = Self::principal_json(access.actor(), access.permissions());
        let (ty, id, attrs) = resource_json(resource);
        self.eval(principal, action.key(), (ty, &id, attrs), json!({"permission": action.key()})).map_err(|reasons| {
            // Same precedence as RBAC: a missing permission is reported before escalation rules.
            if matches!(resource, Resource::Member { .. }) && !access.can(action) {
                return Denied::MissingPermission(action.key());
            }
            Self::first_reason(
                &reasons,
                &[
                    ("escalation", Denied::Escalation),
                    ("owner_only", Denied::MissingPermission(Permission::OrgTransferOwnership.key())),
                ],
            )
            .unwrap_or(match (resource, action) {
                (Resource::Owned { .. }, Permission::RunsManageOwn | Permission::RunsManageAny) => {
                    Denied::MissingPermission(Permission::RunsManageAny.key())
                }
                _ => Denied::MissingPermission(action.key()),
            })
        })
    }

    fn authorize_system(&self, actor: &Actor, action: SystemPermission, ctx: &Context) -> Result<(), Denied> {
        if let Actor::User(u) = actor
            && !u.active
        {
            return Err(Denied::InactiveAccount);
        }
        let principal = Self::principal_json(actor, &PermissionSet::empty());
        let max_age = ctx.max_auth_age.map_or(-1, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        let context = json!({"permission": action.key(), "require_mfa": ctx.require_mfa_for_system, "max_auth_age_secs": max_age});
        match self.eval(principal.clone(), action.key(), ("System", "system", json!({})), context) {
            Ok(()) => Ok(()),
            Err(reasons) => {
                // Lacking the privilege outranks MFA/recency: re-evaluate without conditions.
                let plain = json!({"permission": action.key(), "require_mfa": false, "max_auth_age_secs": -1});
                if self.eval(principal, action.key(), ("System", "system", json!({})), plain).is_err() {
                    return Err(Denied::SystemPrivilegeRequired);
                }
                Err(Self::first_reason(
                    &reasons,
                    &[("mfa_required", Denied::MfaRequired), ("reauth_required", Denied::ReauthenticationRequired)],
                )
                .unwrap_or(Denied::SystemPrivilegeRequired))
            }
        }
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
        let needed = if new_role.is_some() { Permission::MembersUpdateRole } else { Permission::MembersRemove };
        let principal = Self::principal_json(access.actor(), access.permissions());
        let resource = json!({"permissions": current.permissions().keys(), "is_owner": is_owner(current)});
        let context = json!({
            "permission": needed.key(),
            "is_self": is_self,
            "removing": new_role.is_none(),
            "new_permissions": new_role.map(|r| r.permissions().keys()).unwrap_or_default(),
            "new_is_owner": new_role.is_some_and(is_owner),
            "owner_count": i64::try_from(owner_count).unwrap_or(i64::MAX),
        });
        self.eval(principal, "role_change", ("Member", &target_user.to_string(), resource), context).map_err(
            |reasons| {
                if let Some(d) = Self::first_reason(
                    &reasons,
                    &[
                        ("last_owner", Denied::LastOwner),
                        ("self_role_change", Denied::Forbidden("cannot change your own role")),
                    ],
                ) {
                    return d;
                }
                if !access.can(needed) {
                    return Denied::MissingPermission(needed.key());
                }
                Self::first_reason(
                    &reasons,
                    &[
                        ("owner_only", Denied::MissingPermission(Permission::OrgTransferOwnership.key())),
                        ("escalation", Denied::Escalation),
                    ],
                )
                .unwrap_or(Denied::MissingPermission(needed.key()))
            },
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn base_policies_parse_and_project_policies_can_forbid() {
        CedarAuthorizer::new_default().unwrap();
        let dir = std::env::temp_dir().join(format!("cedar-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        // Project rule: API keys may never create runs, whatever their scopes.
        std::fs::write(
            dir.join("project.cedar"),
            r#"@reason("no_key_runs") forbid (principal, action == Action::"runs:create", resource) when { principal.kind == "api_key" };"#,
        )
        .unwrap();
        let engine = CedarAuthorizer::with_policy_dir(&dir).unwrap();
        let org = Uuid::now_v7();
        let key = Actor::ApiKey {
            id: Uuid::now_v7(),
            organization_id: org,
            scopes: PermissionSet::from_iter([Permission::RunsRead, Permission::RunsCreate]),
            created_by: None,
        };
        let facts = MembershipFacts {
            organization_id: org,
            organization_deleted: false,
            role: None,
            creator_role: Some(RoleGrant::builtin(crate::OrgRole::Owner)),
        };
        let access = engine.org_access(&key, &facts).unwrap();
        assert!(access.can(Permission::RunsRead));
        assert!(!access.can(Permission::RunsCreate), "project forbid policy applied");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn malformed_project_policy_is_a_startup_error() {
        let dir = std::env::temp_dir().join(format!("cedar-bad-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bad.cedar"), "permit (principal, action, resource) when {").unwrap();
        assert!(CedarAuthorizer::with_policy_dir(&dir).is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
