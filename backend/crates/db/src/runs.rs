//! Example tenant-owned domain: runs. Reads/writes from requests take `&OrgAccess`; the job
//! worker uses the `job_*` functions, which take the organisation id from the job payload
//! that the API wrote after authorization.

use app_authz::OrgAccess;
use app_domain::{NewRun, Run, RunStatus};
use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    DbError, DbResult,
    pagination::{Cursor, Page, page_from},
};

#[derive(Debug, Clone)]
struct RunRow {
    id: Uuid,
    organization_id: Uuid,
    owner_id: Option<Uuid>,
    label: String,
    provider: String,
    requested: i32,
    succeeded: i32,
    failed: i32,
    status: String,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl From<RunRow> for Run {
    fn from(r: RunRow) -> Run {
        Run {
            id: r.id,
            organization_id: r.organization_id,
            owner_id: r.owner_id.unwrap_or(Uuid::nil()),
            label: r.label,
            provider: r.provider,
            requested: r.requested,
            succeeded: r.succeeded,
            failed: r.failed,
            status: RunStatus::parse(&r.status).unwrap_or(RunStatus::Failed),
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

pub async fn create(db: impl PgExecutor<'_>, access: &OrgAccess, owner: Option<Uuid>, new: &NewRun) -> DbResult<Run> {
    Ok(sqlx::query_as!(
        RunRow,
        r#"INSERT INTO runs (id, organization_id, owner_id, label, provider, requested) VALUES ($1, $2, $3, $4, $5, $6)
           RETURNING id, organization_id, owner_id, label, provider, requested, succeeded, failed, status, created_at, updated_at"#,
        app_domain::new_id(),
        access.org_id(),
        owner,
        new.label,
        new.provider,
        new.requested
    )
    .fetch_one(db)
    .await?
    .into())
}

pub async fn get(db: impl PgExecutor<'_>, access: &OrgAccess, id: Uuid) -> DbResult<Run> {
    Ok(sqlx::query_as!(
        RunRow,
        "SELECT id, organization_id, owner_id, label, provider, requested, succeeded, failed, status, created_at, updated_at
         FROM runs WHERE organization_id = $1 AND id = $2",
        access.org_id(),
        id
    )
    .fetch_optional(db)
    .await?
    .ok_or(DbError::NotFound)?
    .into())
}

pub async fn list(
    db: impl PgExecutor<'_>,
    access: &OrgAccess,
    status: Option<&str>,
    cursor: Option<Cursor>,
    limit: i64,
) -> DbResult<Page<Run>> {
    let rows = sqlx::query_as!(
        RunRow,
        r#"SELECT id, organization_id, owner_id, label, provider, requested, succeeded, failed, status, created_at, updated_at
           FROM runs
           WHERE organization_id = $1 AND ($2::text IS NULL OR status = $2)
             AND ($3::timestamptz IS NULL OR (created_at, id) < ($3, $4))
           ORDER BY created_at DESC, id DESC LIMIT $5"#,
        access.org_id(),
        status,
        cursor.map(|c| c.created_at),
        cursor.map(|c| c.id),
        limit + 1
    )
    .fetch_all(db)
    .await?;
    let runs: Vec<Run> = rows.into_iter().map(Run::from).collect();
    Ok(page_from(runs, limit, |r| Cursor { created_at: r.created_at, id: r.id }))
}

pub async fn delete(db: impl PgExecutor<'_>, access: &OrgAccess, id: Uuid) -> DbResult<()> {
    let r = sqlx::query!("DELETE FROM runs WHERE organization_id = $1 AND id = $2", access.org_id(), id)
        .execute(db)
        .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct RunStats {
    #[ts(type = "number")]
    pub total: i64,
    #[ts(type = "number")]
    pub queued: i64,
    #[ts(type = "number")]
    pub running: i64,
    #[ts(type = "number")]
    pub completed: i64,
    #[ts(type = "number")]
    pub failed: i64,
    #[ts(type = "number")]
    pub calls_succeeded: i64,
    #[ts(type = "number")]
    pub calls_failed: i64,
}

pub async fn stats(db: impl PgExecutor<'_>, access: &OrgAccess, since: OffsetDateTime) -> DbResult<RunStats> {
    let r = sqlx::query!(
        r#"SELECT count(*) AS "total!",
                  count(*) FILTER (WHERE status = 'queued') AS "queued!",
                  count(*) FILTER (WHERE status = 'running') AS "running!",
                  count(*) FILTER (WHERE status = 'completed') AS "completed!",
                  count(*) FILTER (WHERE status = 'failed') AS "failed!",
                  COALESCE(sum(succeeded), 0)::bigint AS "calls_succeeded!",
                  COALESCE(sum(failed), 0)::bigint AS "calls_failed!"
           FROM runs WHERE organization_id = $1 AND created_at >= $2"#,
        access.org_id(),
        since
    )
    .fetch_one(db)
    .await?;
    Ok(RunStats {
        total: r.total,
        queued: r.queued,
        running: r.running,
        completed: r.completed,
        failed: r.failed,
        calls_succeeded: r.calls_succeeded,
        calls_failed: r.calls_failed,
    })
}

/// Daily run counts for a chart (missing days filled with zero).
pub async fn daily_counts(
    db: impl PgExecutor<'_>,
    access: &OrgAccess,
    days: i32,
) -> DbResult<Vec<(time::Date, i64, i64)>> {
    let rows = sqlx::query!(
        r#"SELECT d::date AS "day!", COALESCE(sum(r.succeeded), 0)::bigint AS "ok!", COALESCE(sum(r.failed), 0)::bigint AS "err!"
           FROM generate_series(current_date - ($2::int - 1), current_date, interval '1 day') d
           LEFT JOIN runs r ON r.organization_id = $1 AND r.created_at::date = d::date
           GROUP BY d ORDER BY d"#,
        access.org_id(),
        days
    )
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.day, r.ok, r.err)).collect())
}

// ---- worker-side (organisation id comes from the authorized job payload)

pub async fn job_start(db: impl PgExecutor<'_>, org: Uuid, id: Uuid) -> DbResult<Option<Run>> {
    Ok(sqlx::query_as!(
        RunRow,
        r#"UPDATE runs SET status = 'running' WHERE organization_id = $1 AND id = $2 AND status IN ('queued', 'running')
           RETURNING id, organization_id, owner_id, label, provider, requested, succeeded, failed, status, created_at, updated_at"#,
        org,
        id
    )
    .fetch_optional(db)
    .await?
    .map(Run::from))
}

/// Set absolute progress counters (idempotent under redelivery, unlike increments).
pub async fn job_progress(db: impl PgExecutor<'_>, org: Uuid, id: Uuid, succeeded: i32, failed: i32) -> DbResult<()> {
    sqlx::query!(
        "UPDATE runs SET succeeded = GREATEST(succeeded, $3), failed = GREATEST(failed, $4)
         WHERE organization_id = $1 AND id = $2",
        org,
        id,
        succeeded,
        failed
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn job_finish(db: impl PgExecutor<'_>, org: Uuid, id: Uuid, status: RunStatus) -> DbResult<()> {
    sqlx::query!(
        "UPDATE runs SET status = $3, finished_at = now() WHERE organization_id = $1 AND id = $2",
        org,
        id,
        status.as_str()
    )
    .execute(db)
    .await?;
    Ok(())
}
