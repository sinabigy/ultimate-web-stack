// Base authorization policies for the Cedar engine.
//
// Entity model (built by app-authz/src/cedar.rs from server-verified facts):
//   Principal::"<id>"   { kind: "user"|"service"|"api_key", user_id: String, active: Bool,
//                         system_role: String, mfa: Bool, auth_age_secs: Long,
//                         permissions: Set<String> }   // org permissions from role/scopes
//   Organization::"<id>" {}
//   Owned::"<id>"        { owner: String }               // e.g. a run; "" when ownerless
//   Member::"<user id>"  { permissions: Set<String>, is_owner: Bool }
//   System::"system"     {}
// Actions are Action::"<permission key>" plus Action::"role_change".
// Context always carries `permission` (the permission key being exercised).
//
// Cedar denies by default and `forbid` overrides `permit`. Policies with @reason map denials
// to API error codes. Add project policies as extra *.cedar files (authorization.cedar_policy_dir).

// ---------------------------------------------------------------- organisation permissions
@reason("allow_org")
permit (principal, action, resource is Organization)
when { principal.active && principal.permissions.contains(context.permission) };

// ---------------------------------------------------------------- owned resources
@reason("allow_manage_any")
permit (principal, action in [Action::"runs:manage_own", Action::"runs:manage_any"], resource is Owned)
when { principal.active && principal.permissions.contains("runs:manage_any") };

@reason("allow_manage_own")
permit (principal, action in [Action::"runs:manage_own", Action::"runs:manage_any"], resource is Owned)
when {
    principal.active && principal.kind == "user" && resource.owner != "" &&
    resource.owner == principal.user_id && principal.permissions.contains("runs:manage_own")
};

@reason("allow_owned_generic")
permit (principal, action, resource is Owned)
when {
    principal.active && context.permission != "runs:manage_own" && context.permission != "runs:manage_any" &&
    principal.permissions.contains(context.permission)
};

// ---------------------------------------------------------------- acting on members
@reason("allow_member_action")
permit (principal, action, resource is Member)
when { principal.active && action != Action::"role_change" && principal.permissions.contains(context.permission) };

@reason("owner_only")
forbid (principal, action, resource is Member)
when { action != Action::"role_change" && resource.is_owner && !principal.permissions.contains("org:transfer_ownership") };

@reason("escalation")
forbid (principal, action, resource is Member)
when { action != Action::"role_change" && !principal.permissions.containsAll(resource.permissions) };

// ---------------------------------------------------------------- role changes
@reason("last_owner")
forbid (principal, action == Action::"role_change", resource is Member)
when { resource.is_owner && (context.removing || !context.new_is_owner) && context.owner_count <= 1 };

@reason("allow_leave")
permit (principal, action == Action::"role_change", resource is Member)
when { principal.active && context.is_self && context.removing };

@reason("self_role_change")
forbid (principal, action == Action::"role_change", resource is Member)
when { context.is_self && !context.removing };

@reason("allow_role_change")
permit (principal, action == Action::"role_change", resource is Member)
when { principal.active && !context.is_self && principal.permissions.contains(context.permission) };

@reason("owner_only")
forbid (principal, action == Action::"role_change", resource is Member)
when {
    !context.is_self && (resource.is_owner || context.new_is_owner) &&
    !principal.permissions.contains("org:transfer_ownership")
};

@reason("escalation")
forbid (principal, action == Action::"role_change", resource is Member)
when {
    !context.is_self &&
    !(principal.permissions.containsAll(resource.permissions) && principal.permissions.containsAll(context.new_permissions))
};

// ---------------------------------------------------------------- system administration
@reason("allow_system_admin")
permit (principal, action, resource is System)
when { principal.kind == "user" && principal.active && principal.system_role == "system_admin" };

@reason("allow_system_auditor")
permit (
    principal,
    action in [Action::"system:users:read", Action::"system:orgs:read", Action::"system:audit:read",
               Action::"system:jobs:read", Action::"system:providers:read", Action::"system:health:read",
               Action::"system:roles:read"],
    resource is System
)
when { principal.kind == "user" && principal.active && principal.system_role == "system_auditor" };

@reason("mfa_required")
forbid (principal, action, resource is System)
when { context.require_mfa && !principal.mfa };

@reason("reauth_required")
forbid (principal, action, resource is System)
when { context.max_auth_age_secs >= 0 && principal.auth_age_secs > context.max_auth_age_secs };
