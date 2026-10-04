//! Per-user notifications (in-app). Always scoped by the authenticated user id.

use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DbError, DbResult};

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct NotificationRow {
    pub id: Uuid,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub link: Option<String>,
    pub organization_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    #[ts(type = "string | null")]
    pub read_at: Option<OffsetDateTime>,
}

pub async fn create(
    db: impl PgExecutor<'_>,
    user_id: Uuid,
    org: Option<Uuid>,
    kind: &str,
    title: &str,
    body: &str,
    link: Option<&str>,
) -> DbResult<Uuid> {
    let id = app_domain::new_id();
    sqlx::query!(
        "INSERT INTO notifications (id, user_id, organization_id, kind, title, body, link) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        id,
        user_id,
        org,
        kind,
        title,
        body,
        link
    )
    .execute(db)
    .await?;
    Ok(id)
}

pub async fn list(
    db: impl PgExecutor<'_>,
    user_id: Uuid,
    unread_only: bool,
    limit: i64,
) -> DbResult<Vec<NotificationRow>> {
    Ok(sqlx::query_as!(
        NotificationRow,
        "SELECT id, kind, title, body, link, organization_id, created_at, read_at FROM notifications
         WHERE user_id = $1 AND (NOT $2 OR read_at IS NULL) ORDER BY created_at DESC LIMIT $3",
        user_id,
        unread_only,
        limit
    )
    .fetch_all(db)
    .await?)
}

pub async fn unread_count(db: impl PgExecutor<'_>, user_id: Uuid) -> DbResult<i64> {
    Ok(sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM notifications WHERE user_id = $1 AND read_at IS NULL"#,
        user_id
    )
    .fetch_one(db)
    .await?)
}

pub async fn mark_read(db: impl PgExecutor<'_>, user_id: Uuid, id: Uuid) -> DbResult<()> {
    let r = sqlx::query!(
        "UPDATE notifications SET read_at = COALESCE(read_at, now()) WHERE id = $1 AND user_id = $2",
        id,
        user_id
    )
    .execute(db)
    .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

pub async fn mark_all_read(db: impl PgExecutor<'_>, user_id: Uuid) -> DbResult<u64> {
    Ok(sqlx::query!("UPDATE notifications SET read_at = now() WHERE user_id = $1 AND read_at IS NULL", user_id)
        .execute(db)
        .await?
        .rows_affected())
}
