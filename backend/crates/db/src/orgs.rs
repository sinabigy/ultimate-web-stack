//! Organisations (tenants), memberships, roles, teams and invitations.
//!
//! Everything except membership-fact loading (which is how an `OrgAccess` is obtained) and
//! system-admin listings takes `&OrgAccess` and filters by `access.org_id()`.

use app_authz::{MembershipFacts, OrgAccess, Permission, PermissionSet, RoleGrant, rbac};
use app_domain::{OrgRole, Slug};
use sqlx::{PgConnection, PgExecutor};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DbError, DbResult};

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct OrgRow {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub personal: bool,
    pub settings: serde_json::Value,
    pub billing_plan: String,
    pub billing_customer_ref: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub updated_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    #[ts(type = "string | null")]
    pub deleted_at: Option<OffsetDateTime>,
}

/// Turn a role row into a `RoleGrant`. Built-in roles take permissions from code.
pub fn grant_from(role_id: Uuid, key: &str, builtin: bool, custom: &[String]) -> RoleGrant {
    match (builtin, rbac::builtin_role_for_id(role_id)) {
        (true, Some(r)) => RoleGrant::builtin(r),
        _ => RoleGrant {
            role_id,
            key: key.to_string(),
            builtin: None,
            // Unknown keys are dropped (deny): code is the vocabulary authority.
            custom_permissions: custom.iter().filter_map(|k| Permission::parse(k)).collect(),
        },
    }
}

/// Create an organisation with `owner_id` as its owner. Caller supplies the transaction.
pub async fn create_with_owner(
    conn: &mut PgConnection,
    slug: &Slug,
    name: &str,
    personal: bool,
    owner_id: Uuid,
) -> DbResult<OrgRow> {
    let org = sqlx::query_as!(
        OrgRow,
        r#"INSERT INTO organizations (id, slug, name, personal, created_by) VALUES ($1, $2, $3, $4, $5)
           RETURNING id, slug, name, personal, settings, billing_plan, billing_customer_ref, created_at, updated_at, deleted_at"#,
        app_domain::new_id(),
        slug.as_str(),
        name,
        personal,
        owner_id
    )
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query!(
        "INSERT INTO organization_memberships (organization_id, user_id, role_id) VALUES ($1, $2, $3)",
        org.id,
        owner_id,
        rbac::builtin_role_id(OrgRole::Owner)
    )
    .execute(&mut *conn)
    .await?;
    Ok(org)
}

/// Every user has exactly one personal organisation, created on first login, so all
/// business data is tenant-scoped from day one (enabling multi-org later needs no migration).
pub async fn ensure_personal_org(conn: &mut PgConnection, user_id: Uuid, display_name: &str) -> DbResult<OrgRow> {
    if let Some(org) = sqlx::query_as!(
        OrgRow,
        r#"SELECT id, slug, name, personal, settings, billing_plan, billing_customer_ref, created_at, updated_at, deleted_at
           FROM organizations WHERE personal AND created_by = $1 AND deleted_at IS NULL"#,
        user_id
    )
    .fetch_optional(&mut *conn)
    .await?
    {
        return Ok(org);
    }
    let base = Slug::suggest(display_name);
    for attempt in 0..20u32 {
        // Personal workspaces always carry a suffix so they never claim clean names that a
        // real organisation (company, team) will want later.
        let candidate = format!("{base}-{}", short_suffix(user_id, attempt));
        let Ok(slug) = Slug::parse(&candidate) else { continue };
        // Savepoint so a slug collision does not abort the caller's transaction.
        sqlx::query!("SAVEPOINT personal_org").execute(&mut *conn).await?;
        match create_with_owner(conn, &slug, &format!("{display_name}'s workspace"), true, user_id).await {
            Ok(org) => {
                sqlx::query!("RELEASE SAVEPOINT personal_org").execute(&mut *conn).await?;
                return Ok(org);
            }
            Err(DbError::Conflict(_)) => {
                sqlx::query!("ROLLBACK TO SAVEPOINT personal_org").execute(&mut *conn).await?;
            }
            Err(e) => return Err(e),
        }
    }
    Err(DbError::Conflict("organizations_slug_key".into()))
}

fn short_suffix(id: Uuid, attempt: u32) -> String {
    let n = id.as_u128().wrapping_add(u128::from(attempt) * 7919);
    format!("{:x}", n % 0xff_ffff)
}

#[derive(Debug, Clone)]
pub struct OrgWithFacts {
    pub org: OrgRow,
    pub facts: MembershipFacts,
}

/// Load an organisation by slug together with the user's membership facts.
/// Returns NotFound when the slug does not exist; non-membership is a fact, not an error.
pub async fn facts_for_user_by_slug(db: impl PgExecutor<'_>, user_id: Uuid, slug: &str) -> DbResult<OrgWithFacts> {
    let r = sqlx::query!(
        r#"
        SELECT o.id, o.slug, o.name, o.personal, o.settings, o.billing_plan, o.billing_customer_ref,
               o.created_at, o.updated_at, o.deleted_at,
               m.role_id AS "role_id?", r.key AS "role_key?", r.builtin AS "role_builtin?",
               COALESCE((SELECT array_agg(rp.permission) FROM role_permissions rp WHERE rp.role_id = m.role_id), '{}') AS "custom!: Vec<String>"
        FROM organizations o
        LEFT JOIN organization_memberships m ON m.organization_id = o.id AND m.user_id = $1
        LEFT JOIN roles r ON r.id = m.role_id
        WHERE o.slug = $2
        "#,
        user_id,
        slug
    )
    .fetch_optional(db)
    .await?
    .ok_or(DbError::NotFound)?;
    let role = match (r.role_id, r.role_key, r.role_builtin) {
        (Some(id), Some(k), Some(b)) => Some(grant_from(id, &k, b, &r.custom)),
        _ => None,
    };
    let deleted = r.deleted_at.is_some();
    Ok(OrgWithFacts {
        org: OrgRow {
            id: r.id,
            slug: r.slug,
            name: r.name,
            personal: r.personal,
            settings: r.settings,
            billing_plan: r.billing_plan,
            billing_customer_ref: r.billing_customer_ref,
            created_at: r.created_at,
            updated_at: r.updated_at,
            deleted_at: r.deleted_at,
        },
        facts: MembershipFacts { organization_id: r.id, organization_deleted: deleted, role, creator_role: None },
    })
}

/// Facts for a credential (API key / service) bound to `org_id`; `creator` is the API key's
/// creator whose *current* role bounds the key.
pub async fn facts_for_credential(
    db: impl PgExecutor<'_>,
    org_id: Uuid,
    creator: Option<Uuid>,
) -> DbResult<MembershipFacts> {
    let r = sqlx::query!(
        r#"
        SELECT o.deleted_at, m.role_id AS "role_id?", r.key AS "role_key?", r.builtin AS "role_builtin?",
               COALESCE((SELECT array_agg(rp.permission) FROM role_permissions rp WHERE rp.role_id = m.role_id), '{}') AS "custom!: Vec<String>"
        FROM organizations o
        LEFT JOIN organization_memberships m ON m.organization_id = o.id AND m.user_id = $2
        LEFT JOIN roles r ON r.id = m.role_id
        WHERE o.id = $1
        "#,
        org_id,
        creator
    )
    .fetch_optional(db)
    .await?
    .ok_or(DbError::NotFound)?;
    let creator_role = match (r.role_id, r.role_key, r.role_builtin) {
        (Some(id), Some(k), Some(b)) => Some(grant_from(id, &k, b, &r.custom)),
        _ => None,
    };
    Ok(MembershipFacts {
        organization_id: org_id,
        organization_deleted: r.deleted_at.is_some(),
        role: None,
        creator_role,
    })
}

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct MyOrg {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub personal: bool,
    pub role: String,
    #[ts(type = "number")]
    pub member_count: i64,
}

pub async fn list_for_user(db: impl PgExecutor<'_>, user_id: Uuid) -> DbResult<Vec<MyOrg>> {
    Ok(sqlx::query_as!(
        MyOrg,
        r#"
        SELECT o.id, o.slug, o.name, o.personal, r.key AS role,
               (SELECT count(*) FROM organization_memberships x WHERE x.organization_id = o.id) AS "member_count!"
        FROM organization_memberships m
        JOIN organizations o ON o.id = m.organization_id AND o.deleted_at IS NULL
        JOIN roles r ON r.id = m.role_id
        WHERE m.user_id = $1
        ORDER BY o.personal DESC, o.name
        "#,
        user_id
    )
    .fetch_all(db)
    .await?)
}

pub async fn update(
    db: impl PgExecutor<'_>,
    access: &OrgAccess,
    name: &str,
    settings: &serde_json::Value,
) -> DbResult<OrgRow> {
    sqlx::query_as!(
        OrgRow,
        r#"UPDATE organizations SET name = $2, settings = $3 WHERE id = $1 AND deleted_at IS NULL
           RETURNING id, slug, name, personal, settings, billing_plan, billing_customer_ref, created_at, updated_at, deleted_at"#,
        access.org_id(),
        name,
        settings
    )
    .fetch_optional(db)
    .await?
    .ok_or(DbError::NotFound)
}

pub async fn soft_delete(db: impl PgExecutor<'_>, access: &OrgAccess) -> DbResult<()> {
    let r = sqlx::query!(
        "UPDATE organizations SET deleted_at = now() WHERE id = $1 AND deleted_at IS NULL AND NOT personal",
        access.org_id()
    )
    .execute(db)
    .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

// ------------------------------------------------------------------ members

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS, utoipa::ToSchema)]
#[ts(export)]
pub struct MemberRow {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role_id: Uuid,
    pub role_key: String,
    pub role_name: String,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub joined_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    #[ts(type = "string | null")]
    pub last_login_at: Option<OffsetDateTime>,
}

pub async fn list_members(db: impl PgExecutor<'_>, access: &OrgAccess) -> DbResult<Vec<MemberRow>> {
    Ok(sqlx::query_as!(
        MemberRow,
        r#"
        SELECT u.id AS user_id, u.email, u.display_name, r.id AS role_id, r.key AS role_key, r.name AS role_name,
               m.created_at AS joined_at, u.last_login_at
        FROM organization_memberships m
        JOIN users u ON u.id = m.user_id
        JOIN roles r ON r.id = m.role_id
        WHERE m.organization_id = $1
        ORDER BY m.created_at
        "#,
        access.org_id()
    )
    .fetch_all(db)
    .await?)
}

/// The member's current role grant in this organisation (NotFound if not a member).
pub async fn member_grant(db: impl PgExecutor<'_>, access: &OrgAccess, user_id: Uuid) -> DbResult<RoleGrant> {
    let r = sqlx::query!(
        r#"SELECT r.id, r.key, r.builtin,
                  COALESCE((SELECT array_agg(rp.permission) FROM role_permissions rp WHERE rp.role_id = r.id), '{}') AS "custom!: Vec<String>"
           FROM organization_memberships m JOIN roles r ON r.id = m.role_id
           WHERE m.organization_id = $1 AND m.user_id = $2"#,
        access.org_id(),
        user_id
    )
    .fetch_optional(db)
    .await?
    .ok_or(DbError::NotFound)?;
    Ok(grant_from(r.id, &r.key, r.builtin, &r.custom))
}

/// Lock the organisation's owner rows and count them (serialises concurrent demotions).
pub async fn count_owners_for_update(conn: &mut PgConnection, access: &OrgAccess) -> DbResult<usize> {
    let ids = sqlx::query_scalar!(
        "SELECT user_id FROM organization_memberships WHERE organization_id = $1 AND role_id = $2 FOR UPDATE",
        access.org_id(),
        rbac::builtin_role_id(OrgRole::Owner)
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(ids.len())
}

pub async fn set_member_role(
    db: impl PgExecutor<'_>,
    access: &OrgAccess,
    user_id: Uuid,
    role_id: Uuid,
) -> DbResult<()> {
    let r = sqlx::query!(
        "UPDATE organization_memberships SET role_id = $3 WHERE organization_id = $1 AND user_id = $2",
        access.org_id(),
        user_id,
        role_id
    )
    .execute(db)
    .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

pub async fn remove_member(db: impl PgExecutor<'_>, access: &OrgAccess, user_id: Uuid) -> DbResult<()> {
    let r = sqlx::query!(
        "DELETE FROM organization_memberships WHERE organization_id = $1 AND user_id = $2",
        access.org_id(),
        user_id
    )
    .execute(db)
    .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

/// Organisations where the user is the only owner (blocks account deletion).
pub async fn sole_owner_orgs(db: impl PgExecutor<'_>, user_id: Uuid) -> DbResult<Vec<String>> {
    Ok(sqlx::query_scalar!(
        r#"SELECT o.slug FROM organizations o
           JOIN organization_memberships m ON m.organization_id = o.id AND m.user_id = $1 AND m.role_id = $2
           WHERE o.deleted_at IS NULL AND NOT o.personal
             AND (SELECT count(*) FROM organization_memberships x WHERE x.organization_id = o.id AND x.role_id = $2) = 1"#,
        user_id,
        rbac::builtin_role_id(OrgRole::Owner)
    )
    .fetch_all(db)
    .await?)
}

// ------------------------------------------------------------------ roles

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS, utoipa::ToSchema)]
#[ts(export)]
pub struct RoleRow {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    pub builtin: bool,
    pub permissions: Vec<String>,
    #[ts(type = "number")]
    pub member_count: i64,
}

/// Built-in roles plus this organisation's custom roles, with effective permissions.
pub async fn list_roles(db: impl PgExecutor<'_>, access: &OrgAccess) -> DbResult<Vec<RoleRow>> {
    let rows = sqlx::query!(
        r#"
        SELECT r.id, r.key, r.name, r.description, r.builtin,
               COALESCE((SELECT array_agg(rp.permission ORDER BY rp.permission) FROM role_permissions rp WHERE rp.role_id = r.id), '{}') AS "custom!: Vec<String>",
               (SELECT count(*) FROM organization_memberships m WHERE m.role_id = r.id AND m.organization_id = $1) AS "member_count!"
        FROM roles r
        WHERE r.organization_id IS NULL OR r.organization_id = $1
        ORDER BY r.builtin DESC, r.created_at
        "#,
        access.org_id()
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let grant = grant_from(r.id, &r.key, r.builtin, &r.custom);
            RoleRow {
                id: r.id,
                key: r.key,
                name: r.name,
                description: r.description,
                builtin: r.builtin,
                permissions: grant.permissions().keys().into_iter().map(String::from).collect(),
                member_count: r.member_count,
            }
        })
        .collect())
}

/// Resolve an assignable role (built-in, or custom within this organisation only).
pub async fn role_grant(db: impl PgExecutor<'_>, access: &OrgAccess, role_id: Uuid) -> DbResult<RoleGrant> {
    let r = sqlx::query!(
        r#"SELECT r.id, r.key, r.builtin,
                  COALESCE((SELECT array_agg(rp.permission) FROM role_permissions rp WHERE rp.role_id = r.id), '{}') AS "custom!: Vec<String>"
           FROM roles r WHERE r.id = $1 AND (r.organization_id IS NULL OR r.organization_id = $2)"#,
        role_id,
        access.org_id()
    )
    .fetch_optional(db)
    .await?
    .ok_or(DbError::NotFound)?;
    Ok(grant_from(r.id, &r.key, r.builtin, &r.custom))
}

pub async fn create_custom_role(
    conn: &mut PgConnection,
    access: &OrgAccess,
    key: &str,
    name: &str,
    description: &str,
    permissions: &PermissionSet,
) -> DbResult<Uuid> {
    let id = app_domain::new_id();
    sqlx::query!(
        "INSERT INTO roles (id, organization_id, key, name, description, builtin) VALUES ($1, $2, $3, $4, $5, false)",
        id,
        access.org_id(),
        key,
        name,
        description
    )
    .execute(&mut *conn)
    .await?;
    let keys: Vec<String> = permissions.keys().into_iter().map(String::from).collect();
    sqlx::query!("INSERT INTO role_permissions (role_id, permission) SELECT $1, unnest($2::text[])", id, &keys)
        .execute(&mut *conn)
        .await?;
    Ok(id)
}

/// Delete a custom role of this organisation. Fails with Conflict while assigned.
pub async fn delete_custom_role(db: impl PgExecutor<'_>, access: &OrgAccess, role_id: Uuid) -> DbResult<()> {
    let r = sqlx::query!(
        "DELETE FROM roles WHERE id = $1 AND organization_id = $2 AND NOT builtin",
        role_id,
        access.org_id()
    )
    .execute(db)
    .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

/// Mirror the code-defined permission vocabulary into the `permissions` table (startup).
pub async fn sync_permissions(db: impl PgExecutor<'_>) -> DbResult<()> {
    let keys: Vec<String> = Permission::ALL.iter().map(|p| p.key().to_string()).collect();
    let descs: Vec<String> = Permission::ALL.iter().map(|p| p.description().to_string()).collect();
    sqlx::query!(
        "INSERT INTO permissions (key, description) SELECT * FROM unnest($1::text[], $2::text[])
         ON CONFLICT (key) DO UPDATE SET description = EXCLUDED.description",
        &keys,
        &descs
    )
    .execute(db)
    .await?;
    Ok(())
}

// ------------------------------------------------------------------ teams

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS, utoipa::ToSchema)]
#[ts(export)]
pub struct TeamRow {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    #[ts(type = "number")]
    pub member_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub created_at: OffsetDateTime,
}

pub async fn list_teams(db: impl PgExecutor<'_>, access: &OrgAccess) -> DbResult<Vec<TeamRow>> {
    Ok(sqlx::query_as!(
        TeamRow,
        r#"SELECT t.id, t.name, t.description, t.created_at,
                  (SELECT count(*) FROM team_members tm WHERE tm.team_id = t.id) AS "member_count!"
           FROM teams t WHERE t.organization_id = $1 ORDER BY t.name"#,
        access.org_id()
    )
    .fetch_all(db)
    .await?)
}

pub async fn create_team(db: impl PgExecutor<'_>, access: &OrgAccess, name: &str, description: &str) -> DbResult<Uuid> {
    let id = app_domain::new_id();
    sqlx::query!(
        "INSERT INTO teams (id, organization_id, name, description) VALUES ($1, $2, $3, $4)",
        id,
        access.org_id(),
        name,
        description
    )
    .execute(db)
    .await?;
    Ok(id)
}

pub async fn delete_team(db: impl PgExecutor<'_>, access: &OrgAccess, team_id: Uuid) -> DbResult<()> {
    let r = sqlx::query!("DELETE FROM teams WHERE id = $1 AND organization_id = $2", team_id, access.org_id())
        .execute(db)
        .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

/// Add a member to a team. Both team and user must belong to this organisation; the
/// composite foreign key rejects users who are not members.
pub async fn add_team_member(
    db: impl PgExecutor<'_>,
    access: &OrgAccess,
    team_id: Uuid,
    user_id: Uuid,
) -> DbResult<()> {
    let r = sqlx::query!(
        "INSERT INTO team_members (team_id, organization_id, user_id)
         SELECT id, organization_id, $3 FROM teams WHERE id = $1 AND organization_id = $2
         ON CONFLICT DO NOTHING",
        team_id,
        access.org_id(),
        user_id
    )
    .execute(db)
    .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

pub async fn remove_team_member(
    db: impl PgExecutor<'_>,
    access: &OrgAccess,
    team_id: Uuid,
    user_id: Uuid,
) -> DbResult<()> {
    let r = sqlx::query!(
        "DELETE FROM team_members WHERE team_id = $1 AND organization_id = $2 AND user_id = $3",
        team_id,
        access.org_id(),
        user_id
    )
    .execute(db)
    .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

pub async fn list_team_members(db: impl PgExecutor<'_>, access: &OrgAccess, team_id: Uuid) -> DbResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar!(
        "SELECT user_id FROM team_members WHERE team_id = $1 AND organization_id = $2 ORDER BY created_at",
        team_id,
        access.org_id()
    )
    .fetch_all(db)
    .await?)
}

// ------------------------------------------------------------------ invitations

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct InvitationRow {
    pub id: Uuid,
    pub email: String,
    pub role_id: Uuid,
    pub role_key: String,
    pub invited_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub expires_at: OffsetDateTime,
}

pub async fn create_invitation(
    db: impl PgExecutor<'_>,
    access: &OrgAccess,
    email: &str,
    role_id: Uuid,
    token_hash: &[u8],
    invited_by: Uuid,
    expires_at: OffsetDateTime,
) -> DbResult<Uuid> {
    let id = app_domain::new_id();
    sqlx::query!(
        "INSERT INTO invitations (id, organization_id, email, role_id, token_hash, invited_by, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
        id,
        access.org_id(),
        email,
        role_id,
        token_hash,
        invited_by,
        expires_at
    )
    .execute(db)
    .await?;
    Ok(id)
}

pub async fn list_pending_invitations(db: impl PgExecutor<'_>, access: &OrgAccess) -> DbResult<Vec<InvitationRow>> {
    Ok(sqlx::query_as!(
        InvitationRow,
        r#"SELECT i.id, i.email, i.role_id, r.key AS role_key, i.invited_by, i.created_at, i.expires_at
           FROM invitations i JOIN roles r ON r.id = i.role_id
           WHERE i.organization_id = $1 AND i.accepted_at IS NULL AND i.revoked_at IS NULL AND i.expires_at > now()
           ORDER BY i.created_at DESC"#,
        access.org_id()
    )
    .fetch_all(db)
    .await?)
}

pub async fn revoke_invitation(db: impl PgExecutor<'_>, access: &OrgAccess, id: Uuid) -> DbResult<()> {
    let r = sqlx::query!(
        "UPDATE invitations SET revoked_at = now() WHERE id = $1 AND organization_id = $2 AND accepted_at IS NULL AND revoked_at IS NULL",
        id,
        access.org_id()
    )
    .execute(db)
    .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct InvitationPreview {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub organization_slug: String,
    pub organization_name: String,
    pub email: String,
    pub role_id: Uuid,
    pub role_key: String,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub expires_at: OffsetDateTime,
}

/// Look up a *pending* invitation by token hash (for the accept page).
pub async fn find_invitation(db: impl PgExecutor<'_>, token_hash: &[u8]) -> DbResult<InvitationPreview> {
    sqlx::query_as!(
        InvitationPreview,
        r#"SELECT i.id, i.organization_id, o.slug AS organization_slug, o.name AS organization_name, i.email,
                  i.role_id, r.key AS role_key, i.expires_at
           FROM invitations i JOIN organizations o ON o.id = i.organization_id AND o.deleted_at IS NULL
           JOIN roles r ON r.id = i.role_id
           WHERE i.token_hash = $1 AND i.accepted_at IS NULL AND i.revoked_at IS NULL AND i.expires_at > now()"#,
        token_hash
    )
    .fetch_optional(db)
    .await?
    .ok_or(DbError::NotFound)
}

/// Accept: mark the invitation used and create the membership atomically (caller's tx).
/// Single use is enforced by the `accepted_at IS NULL` predicate under row lock.
pub async fn accept_invitation(conn: &mut PgConnection, invitation_id: Uuid, user_id: Uuid) -> DbResult<Uuid> {
    let inv = sqlx::query!(
        "UPDATE invitations SET accepted_at = now(), accepted_by = $2
         WHERE id = $1 AND accepted_at IS NULL AND revoked_at IS NULL AND expires_at > now()
         RETURNING organization_id, role_id, invited_by",
        invitation_id,
        user_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(DbError::NotFound)?;
    sqlx::query!(
        "INSERT INTO organization_memberships (organization_id, user_id, role_id, invited_by) VALUES ($1, $2, $3, $4)
         ON CONFLICT (organization_id, user_id) DO NOTHING",
        inv.organization_id,
        user_id,
        inv.role_id,
        inv.invited_by
    )
    .execute(&mut *conn)
    .await?;
    Ok(inv.organization_id)
}

// ------------------------------------------------------------------ system admin

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AdminOrgRow {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub personal: bool,
    pub billing_plan: String,
    #[ts(type = "number")]
    pub member_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    #[ts(type = "string | null")]
    pub deleted_at: Option<OffsetDateTime>,
}

pub async fn admin_list(
    db: impl PgExecutor<'_>,
    search: Option<&str>,
    limit: i64,
    offset: i64,
) -> DbResult<(Vec<AdminOrgRow>, i64)> {
    let pattern = search.map(|s| format!("%{}%", s.replace(['%', '_', '\\'], "")));
    let rows = sqlx::query!(
        r#"SELECT o.id, o.slug, o.name, o.personal, o.billing_plan, o.created_at, o.deleted_at,
                  (SELECT count(*) FROM organization_memberships m WHERE m.organization_id = o.id) AS "member_count!",
                  count(*) OVER () AS "total!"
           FROM organizations o
           WHERE ($1::text IS NULL OR o.slug ILIKE $1 OR o.name ILIKE $1)
           ORDER BY o.created_at DESC, o.id DESC LIMIT $2 OFFSET $3"#,
        pattern,
        limit,
        offset
    )
    .fetch_all(db)
    .await?;
    let total = rows.first().map_or(0, |r| r.total);
    Ok((
        rows.into_iter()
            .map(|r| AdminOrgRow {
                id: r.id,
                slug: r.slug,
                name: r.name,
                personal: r.personal,
                billing_plan: r.billing_plan,
                member_count: r.member_count,
                created_at: r.created_at,
                deleted_at: r.deleted_at,
            })
            .collect(),
        total,
    ))
}
