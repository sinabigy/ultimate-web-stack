//! API keys and machine-to-machine service clients.

use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DbError, DbResult};
use app_authz::OrgAccess;

#[derive(Debug, Clone)]
pub struct ApiKeyAuthRow {
    pub id: Uuid,
    pub secret_hash: Vec<u8>,
    pub organization_id: Uuid,
    pub created_by: Option<Uuid>,
    pub scopes: Vec<String>,
    pub name: String,
    pub last_used_at: Option<OffsetDateTime>,
}

/// Lookup for authentication: only keys that are not revoked and not expired.
pub async fn find_active(db: impl PgExecutor<'_>, key_id: &str) -> DbResult<Option<ApiKeyAuthRow>> {
    Ok(sqlx::query_as!(
        ApiKeyAuthRow,
        r#"SELECT id, secret_hash, organization_id, created_by, scopes, name, last_used_at FROM api_keys
           WHERE key_id = $1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())"#,
        key_id
    )
    .fetch_optional(db)
    .await?)
}

/// Update `last_used_at` at most once a minute per key (avoids a write per request).
pub async fn touch(db: impl PgExecutor<'_>, id: Uuid) -> DbResult<()> {
    sqlx::query!(
        "UPDATE api_keys SET last_used_at = now() WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')",
        id
    )
    .execute(db)
    .await?;
    Ok(())
}

pub struct NewApiKey<'a> {
    pub key_id: &'a str,
    pub secret_hash: &'a [u8],
    pub name: &'a str,
    pub created_by: Uuid,
    pub scopes: &'a [String],
    pub expires_at: Option<OffsetDateTime>,
    pub rotated_from: Option<Uuid>,
}

pub async fn insert(db: impl PgExecutor<'_>, access: &OrgAccess, k: &NewApiKey<'_>) -> DbResult<Uuid> {
    let id = app_domain::new_id();
    sqlx::query!(
        "INSERT INTO api_keys (id, key_id, secret_hash, name, organization_id, created_by, scopes, expires_at, rotated_from)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        id,
        k.key_id,
        k.secret_hash,
        k.name,
        access.org_id(),
        k.created_by,
        k.scopes,
        k.expires_at,
        k.rotated_from
    )
    .execute(db)
    .await?;
    Ok(id)
}

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ApiKeyRow {
    pub id: Uuid,
    pub key_id: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub created_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    #[ts(type = "string | null")]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[ts(type = "string | null")]
    pub last_used_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[ts(type = "string | null")]
    pub revoked_at: Option<OffsetDateTime>,
}

pub async fn list(db: impl PgExecutor<'_>, access: &OrgAccess) -> DbResult<Vec<ApiKeyRow>> {
    Ok(sqlx::query_as!(
        ApiKeyRow,
        "SELECT id, key_id, name, scopes, created_by, created_at, expires_at, last_used_at, revoked_at
         FROM api_keys WHERE organization_id = $1 ORDER BY created_at DESC LIMIT 500",
        access.org_id()
    )
    .fetch_all(db)
    .await?)
}

pub async fn get(db: impl PgExecutor<'_>, access: &OrgAccess, id: Uuid) -> DbResult<ApiKeyRow> {
    sqlx::query_as!(
        ApiKeyRow,
        "SELECT id, key_id, name, scopes, created_by, created_at, expires_at, last_used_at, revoked_at
         FROM api_keys WHERE organization_id = $1 AND id = $2",
        access.org_id(),
        id
    )
    .fetch_optional(db)
    .await?
    .ok_or(DbError::NotFound)
}

pub async fn revoke(db: impl PgExecutor<'_>, access: &OrgAccess, id: Uuid) -> DbResult<()> {
    let r = sqlx::query!(
        "UPDATE api_keys SET revoked_at = now() WHERE organization_id = $1 AND id = $2 AND revoked_at IS NULL",
        access.org_id(),
        id
    )
    .execute(db)
    .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

#[derive(Debug, Clone)]
pub struct ServiceClientRow {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub name: String,
    pub scopes: Vec<String>,
}

/// Active service client for a token subject (`sub` / `client_id` from the IdP).
pub async fn find_service_client(db: impl PgExecutor<'_>, subject: &str) -> DbResult<Option<ServiceClientRow>> {
    Ok(sqlx::query_as!(
        ServiceClientRow,
        "SELECT id, organization_id, name, scopes FROM service_clients WHERE subject = $1 AND disabled_at IS NULL",
        subject
    )
    .fetch_optional(db)
    .await?)
}

pub async fn register_service_client(
    db: impl PgExecutor<'_>,
    access: &OrgAccess,
    subject: &str,
    name: &str,
    scopes: &[String],
    created_by: Uuid,
) -> DbResult<Uuid> {
    let id = app_domain::new_id();
    sqlx::query!(
        "INSERT INTO service_clients (id, organization_id, subject, name, scopes, created_by) VALUES ($1, $2, $3, $4, $5, $6)",
        id,
        access.org_id(),
        subject,
        name,
        scopes,
        created_by
    )
    .execute(db)
    .await?;
    Ok(id)
}

/// Rotation overlap: let the old key keep working until `at` (bounded by any earlier expiry).
pub async fn expire_at(db: impl PgExecutor<'_>, access: &OrgAccess, id: Uuid, at: OffsetDateTime) -> DbResult<()> {
    let r = sqlx::query!(
        "UPDATE api_keys SET expires_at = LEAST(COALESCE(expires_at, $3), $3) WHERE organization_id = $1 AND id = $2 AND revoked_at IS NULL",
        access.org_id(),
        id,
        at
    )
    .execute(db)
    .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}
