//! Users: identity-provider references plus application-owned profile data.

use app_domain::{Email, SystemRole, User, UserStatus};
use sqlx::{PgConnection, PgExecutor};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DbError, DbResult};

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct UserRow {
    pub id: Uuid,
    pub identity_provider: String,
    pub external_subject: String,
    pub email: String,
    pub email_verified: bool,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub preferences: serde_json::Value,
    pub status: String,
    pub system_role: String,
    #[serde(with = "time::serde::rfc3339::option")]
    #[ts(type = "string | null")]
    pub last_login_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub updated_at: OffsetDateTime,
}

impl UserRow {
    pub fn status(&self) -> UserStatus {
        UserStatus::parse(&self.status).unwrap_or(UserStatus::Suspended) // unknown → least privilege
    }
    pub fn system_role(&self) -> SystemRole {
        SystemRole::parse(&self.system_role).unwrap_or(SystemRole::None)
    }
    pub fn to_domain(&self) -> Option<User> {
        Some(User {
            id: self.id,
            email: Email::parse(&self.email).ok()?,
            email_verified: self.email_verified,
            display_name: self.display_name.clone(),
            status: self.status(),
            system_role: self.system_role(),
            created_at: self.created_at,
        })
    }
}

/// Verified identity claims from an ID token.
#[derive(Debug, Clone)]
pub struct IdentityClaims<'a> {
    pub issuer: &'a str,
    pub subject: &'a str,
    pub email: &'a str,
    pub email_verified: bool,
    pub name: Option<&'a str>,
    pub picture: Option<&'a str>,
}

/// Insert or refresh the local user for a login. Returns `(user, created)`.
/// The identity key is `(issuer, sub)`; email is cached, never used for matching.
/// `display_name` is application-owned after creation and is not overwritten.
/// `bootstrap_admin` upgrades `none` → `system_admin` (never downgrades).
pub async fn upsert_from_login(
    db: impl PgExecutor<'_>,
    claims: &IdentityClaims<'_>,
    bootstrap_admin: bool,
) -> DbResult<(UserRow, bool)> {
    let display = claims
        .name
        .filter(|n| !n.trim().is_empty())
        .map(|n| n.trim().chars().take(80).collect::<String>())
        .unwrap_or_else(|| claims.email.split('@').next().unwrap_or("User").to_string());
    let rec = sqlx::query!(
        r#"
        INSERT INTO users (id, identity_provider, external_subject, email, email_verified, display_name, avatar_url,
                           system_role, last_login_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, CASE WHEN $8 THEN 'system_admin' ELSE 'none' END, now())
        ON CONFLICT (identity_provider, external_subject) DO UPDATE SET
            email = EXCLUDED.email,
            email_verified = EXCLUDED.email_verified,
            avatar_url = COALESCE(users.avatar_url, EXCLUDED.avatar_url),
            last_login_at = now(),
            system_role = CASE WHEN $8 AND users.system_role = 'none' THEN 'system_admin' ELSE users.system_role END
        RETURNING id, identity_provider, external_subject, email, email_verified, display_name, avatar_url,
                  preferences, status, system_role, last_login_at, created_at, updated_at, (xmax = 0) AS "inserted!"
        "#,
        app_domain::new_id(),
        claims.issuer,
        claims.subject,
        claims.email,
        claims.email_verified,
        display,
        claims.picture,
        bootstrap_admin,
    )
    .fetch_one(db)
    .await?;
    let row = UserRow {
        id: rec.id,
        identity_provider: rec.identity_provider,
        external_subject: rec.external_subject,
        email: rec.email,
        email_verified: rec.email_verified,
        display_name: rec.display_name,
        avatar_url: rec.avatar_url,
        preferences: rec.preferences,
        status: rec.status,
        system_role: rec.system_role,
        last_login_at: rec.last_login_at,
        created_at: rec.created_at,
        updated_at: rec.updated_at,
    };
    Ok((row, rec.inserted))
}

pub async fn get(db: impl PgExecutor<'_>, id: Uuid) -> DbResult<UserRow> {
    sqlx::query_as!(UserRow, "SELECT id, identity_provider, external_subject, email, email_verified, display_name, avatar_url, preferences, status, system_role, last_login_at, created_at, updated_at FROM users WHERE id = $1", id)
        .fetch_optional(db)
        .await?
        .ok_or(DbError::NotFound)
}

pub async fn update_profile(
    db: impl PgExecutor<'_>,
    id: Uuid,
    display_name: &str,
    avatar_url: Option<&str>,
) -> DbResult<UserRow> {
    sqlx::query_as!(
        UserRow,
        "UPDATE users SET display_name = $2, avatar_url = $3 WHERE id = $1 AND status <> 'deleted'
         RETURNING id, identity_provider, external_subject, email, email_verified, display_name, avatar_url, preferences, status, system_role, last_login_at, created_at, updated_at",
        id,
        display_name,
        avatar_url
    )
    .fetch_optional(db)
    .await?
    .ok_or(DbError::NotFound)
}

pub async fn update_preferences(db: impl PgExecutor<'_>, id: Uuid, prefs: &serde_json::Value) -> DbResult<()> {
    let r = sqlx::query!("UPDATE users SET preferences = $2 WHERE id = $1 AND status <> 'deleted'", id, prefs)
        .execute(db)
        .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AdminUserRow {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub status: String,
    pub system_role: String,
    pub email_verified: bool,
    #[ts(type = "number")]
    pub org_count: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    #[ts(type = "string | null")]
    pub last_login_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub created_at: OffsetDateTime,
}

/// System-admin listing (caller must hold `system:users:read`).
pub async fn admin_list(
    db: impl PgExecutor<'_>,
    search: Option<&str>,
    status: Option<&str>,
    limit: i64,
    offset: i64,
) -> DbResult<(Vec<AdminUserRow>, i64)> {
    let pattern = search.map(|s| format!("%{}%", s.replace(['%', '_', '\\'], "")));
    let rows = sqlx::query!(
        r#"
        SELECT u.id, u.email, u.display_name, u.status, u.system_role, u.email_verified, u.last_login_at, u.created_at,
               (SELECT count(*) FROM organization_memberships m WHERE m.user_id = u.id) AS "org_count!",
               count(*) OVER () AS "total!"
        FROM users u
        WHERE ($1::text IS NULL OR u.email ILIKE $1 OR u.display_name ILIKE $1)
          AND ($2::text IS NULL OR u.status = $2)
        ORDER BY u.created_at DESC, u.id DESC
        LIMIT $3 OFFSET $4
        "#,
        pattern,
        status,
        limit,
        offset
    )
    .fetch_all(db)
    .await?;
    let total = rows.first().map_or(0, |r| r.total);
    Ok((
        rows.into_iter()
            .map(|r| AdminUserRow {
                id: r.id,
                email: r.email,
                display_name: r.display_name,
                status: r.status,
                system_role: r.system_role,
                email_verified: r.email_verified,
                org_count: r.org_count,
                last_login_at: r.last_login_at,
                created_at: r.created_at,
            })
            .collect(),
        total,
    ))
}

pub async fn set_status(db: impl PgExecutor<'_>, id: Uuid, status: UserStatus) -> DbResult<()> {
    let r = sqlx::query!("UPDATE users SET status = $2 WHERE id = $1 AND status <> 'deleted'", id, status.as_str())
        .execute(db)
        .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

pub async fn set_system_role(db: impl PgExecutor<'_>, id: Uuid, role: SystemRole) -> DbResult<()> {
    let r = sqlx::query!("UPDATE users SET system_role = $2 WHERE id = $1 AND status <> 'deleted'", id, role.as_str())
        .execute(db)
        .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

pub async fn count_system_admins(db: impl PgExecutor<'_>) -> DbResult<i64> {
    Ok(sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM users WHERE system_role = 'system_admin' AND status = 'active'"#
    )
    .fetch_one(db)
    .await?)
}

/// Soft-delete with PII scrubbing. The IdP subject is released so the same identity can sign
/// up again as a *new* user. Sessions are revoked by the caller in the same transaction.
pub async fn mark_deleted(conn: &mut PgConnection, id: Uuid) -> DbResult<()> {
    let r = sqlx::query!(
        r#"
        UPDATE users SET status = 'deleted', email = 'deleted+' || id::text || '@invalid',
               external_subject = 'deleted:' || id::text, display_name = 'Deleted user', avatar_url = NULL,
               preferences = '{}'::jsonb, system_role = 'none', deleted_at = now()
        WHERE id = $1 AND status <> 'deleted'
        "#,
        id
    )
    .execute(&mut *conn)
    .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SystemCounts {
    #[ts(type = "number")]
    pub users: i64,
    #[ts(type = "number")]
    pub active_users_7d: i64,
    #[ts(type = "number")]
    pub organizations: i64,
    #[ts(type = "number")]
    pub active_sessions: i64,
    #[ts(type = "number")]
    pub denied_24h: i64,
    #[ts(type = "number")]
    pub system_admins: i64,
}

pub async fn system_counts(db: impl PgExecutor<'_>) -> DbResult<SystemCounts> {
    let r = sqlx::query!(
        r#"SELECT
             (SELECT count(*) FROM users WHERE status <> 'deleted') AS "users!",
             (SELECT count(*) FROM users WHERE last_login_at > now() - interval '7 days') AS "active_users_7d!",
             (SELECT count(*) FROM organizations WHERE deleted_at IS NULL) AS "organizations!",
             (SELECT count(*) FROM sessions WHERE revoked_at IS NULL AND expires_at > now() AND idle_expires_at > now()) AS "active_sessions!",
             (SELECT count(*) FROM audit_events WHERE outcome = 'denied' AND occurred_at > now() - interval '24 hours') AS "denied_24h!",
             (SELECT count(*) FROM users WHERE system_role = 'system_admin' AND status = 'active') AS "system_admins!""#
    )
    .fetch_one(db)
    .await?;
    Ok(SystemCounts {
        users: r.users,
        active_users_7d: r.active_users_7d,
        organizations: r.organizations,
        active_sessions: r.active_sessions,
        denied_24h: r.denied_24h,
        system_admins: r.system_admins,
    })
}
