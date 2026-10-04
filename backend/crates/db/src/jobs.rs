//! PostgreSQL job queue (core profile). At-least-once delivery with leases:
//!
//! - `enqueue` is idempotent per `idempotency_key`.
//! - `claim` uses `FOR UPDATE SKIP LOCKED` so many workers never contend on one row, and sets
//!   a lease (`locked_until`). A crashed worker's lease expires and the job is reclaimed.
//! - `fail` retries with exponential backoff until `max_attempts`, then marks the job `dead`
//!   (the dead-letter state; visible in /admin/jobs and retryable by an admin).
//!
//! Handlers must therefore be idempotent.

use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DbError, DbResult};

#[derive(Debug, Clone, serde::Serialize)]
pub struct JobRow {
    pub id: Uuid,
    pub queue: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub status: String,
    pub priority: i16,
    pub attempts: i32,
    pub max_attempts: i32,
    pub run_at: OffsetDateTime,
    pub last_error: Option<String>,
    pub organization_id: Option<Uuid>,
    pub trace_context: Option<serde_json::Value>,
    pub created_at: OffsetDateTime,
    pub finished_at: Option<OffsetDateTime>,
}

pub struct NewJob<'a> {
    pub queue: &'a str,
    pub kind: &'a str,
    pub payload: serde_json::Value,
    pub priority: i16,
    pub max_attempts: i32,
    pub run_at: Option<OffsetDateTime>,
    pub idempotency_key: Option<&'a str>,
    pub organization_id: Option<Uuid>,
    pub trace_context: Option<serde_json::Value>,
}

/// Returns `(job_id, created)`. A duplicate idempotency key returns the existing job.
/// One statement, so it composes with the caller's transaction (transactional outbox:
/// the business row and its job commit or roll back together).
pub async fn enqueue(db: impl PgExecutor<'_>, j: &NewJob<'_>) -> DbResult<(Uuid, bool)> {
    let row = sqlx::query!(
        r#"
        WITH ins AS (
            INSERT INTO jobs (id, queue, kind, payload, priority, max_attempts, run_at, idempotency_key, organization_id, trace_context)
            VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, now()), $8, $9, $10)
            ON CONFLICT (idempotency_key) DO NOTHING
            RETURNING id
        )
        SELECT id AS "id!", true AS "created!" FROM ins
        UNION ALL
        SELECT id, false FROM jobs WHERE idempotency_key = $8 AND NOT EXISTS (SELECT 1 FROM ins)
        "#,
        app_domain::new_id(),
        j.queue,
        j.kind,
        j.payload,
        j.priority,
        j.max_attempts,
        j.run_at,
        j.idempotency_key,
        j.organization_id,
        j.trace_context
    )
    .fetch_optional(db)
    .await?
    .ok_or(DbError::Conflict("jobs".into()))?;
    Ok((row.id, row.created))
}

/// Claim up to `limit` ready jobs for `worker`, leasing them for `lease_secs`.
pub async fn claim(
    db: impl PgExecutor<'_>,
    queue: &str,
    worker: &str,
    limit: i64,
    lease_secs: f64,
) -> DbResult<Vec<JobRow>> {
    Ok(sqlx::query_as!(
        JobRow,
        r#"
        UPDATE jobs SET status = 'running', attempts = attempts + 1, locked_by = $2,
                        locked_until = now() + make_interval(secs => $4)
        WHERE id IN (
            SELECT id FROM jobs WHERE queue = $1 AND status = 'queued' AND run_at <= now()
            ORDER BY priority DESC, run_at LIMIT $3 FOR UPDATE SKIP LOCKED
        )
        RETURNING id, queue, kind, payload, status, priority, attempts, max_attempts, run_at, last_error,
                  organization_id, trace_context, created_at, finished_at
        "#,
        queue,
        worker,
        limit,
        lease_secs
    )
    .fetch_all(db)
    .await?)
}

/// Extend a lease for long-running jobs (heartbeat). False if the lease was lost.
pub async fn heartbeat(db: impl PgExecutor<'_>, id: Uuid, worker: &str, lease_secs: f64) -> DbResult<bool> {
    let r = sqlx::query!(
        "UPDATE jobs SET locked_until = now() + make_interval(secs => $3) WHERE id = $1 AND locked_by = $2 AND status = 'running'",
        id,
        worker,
        lease_secs
    )
    .execute(db)
    .await?;
    Ok(r.rows_affected() == 1)
}

pub async fn complete(db: impl PgExecutor<'_>, id: Uuid, worker: &str) -> DbResult<bool> {
    let r = sqlx::query!(
        "UPDATE jobs SET status = 'succeeded', finished_at = now(), locked_by = NULL, locked_until = NULL
         WHERE id = $1 AND locked_by = $2 AND status = 'running'",
        id,
        worker
    )
    .execute(db)
    .await?;
    Ok(r.rows_affected() == 1)
}

/// Record a failure: requeue with exponential backoff (2^attempts seconds, capped at 1h, plus
/// up to 25% jitter computed in SQL) or move to `dead` when attempts are exhausted or `permanent`.
/// Returns the new status.
pub async fn fail(db: impl PgExecutor<'_>, id: Uuid, worker: &str, error: &str, permanent: bool) -> DbResult<String> {
    let err: String = error.chars().take(2000).collect();
    let status = sqlx::query_scalar!(
        r#"UPDATE jobs SET
             status = CASE WHEN $4 OR attempts >= max_attempts THEN 'dead' ELSE 'queued' END,
             run_at = now() + make_interval(secs => LEAST(3600, power(2, attempts)) * (1 + random() * 0.25)),
             finished_at = CASE WHEN $4 OR attempts >= max_attempts THEN now() ELSE NULL END,
             last_error = $3, locked_by = NULL, locked_until = NULL
           WHERE id = $1 AND locked_by = $2 AND status = 'running'
           RETURNING status"#,
        id,
        worker,
        err,
        permanent
    )
    .fetch_optional(db)
    .await?
    .ok_or(DbError::NotFound)?;
    Ok(status)
}

/// Return jobs whose lease expired (worker crashed) to the queue. Returns how many.
pub async fn reap_expired(db: impl PgExecutor<'_>) -> DbResult<u64> {
    Ok(sqlx::query!(
        "UPDATE jobs SET status = 'queued', locked_by = NULL, locked_until = NULL, last_error = 'lease expired'
         WHERE status = 'running' AND locked_until < now()"
    )
    .execute(db)
    .await?
    .rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct QueueStats {
    pub queue: String,
    pub queued: i64,
    pub running: i64,
    pub dead: i64,
    pub succeeded_24h: i64,
    pub oldest_queued_secs: Option<f64>,
}

pub async fn stats(db: impl PgExecutor<'_>) -> DbResult<Vec<QueueStats>> {
    let rows = sqlx::query!(
        r#"SELECT queue,
                  count(*) FILTER (WHERE status = 'queued') AS "queued!",
                  count(*) FILTER (WHERE status = 'running') AS "running!",
                  count(*) FILTER (WHERE status = 'dead') AS "dead!",
                  count(*) FILTER (WHERE status = 'succeeded' AND finished_at > now() - interval '24 hours') AS "succeeded_24h!",
                  EXTRACT(EPOCH FROM now() - min(run_at) FILTER (WHERE status = 'queued'))::float8 AS oldest_queued_secs
           FROM jobs GROUP BY queue ORDER BY queue"#
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| QueueStats {
            queue: r.queue,
            queued: r.queued,
            running: r.running,
            dead: r.dead,
            succeeded_24h: r.succeeded_24h,
            oldest_queued_secs: r.oldest_queued_secs,
        })
        .collect())
}

pub async fn list(db: impl PgExecutor<'_>, status: Option<&str>, limit: i64) -> DbResult<Vec<JobRow>> {
    Ok(sqlx::query_as!(
        JobRow,
        r#"SELECT id, queue, kind, payload, status, priority, attempts, max_attempts, run_at, last_error, organization_id,
                  trace_context, created_at, finished_at
           FROM jobs WHERE ($1::text IS NULL OR status = $1) ORDER BY created_at DESC LIMIT $2"#,
        status,
        limit
    )
    .fetch_all(db)
    .await?)
}

/// Admin action: requeue a dead job with a fresh attempt budget.
pub async fn retry_dead(db: impl PgExecutor<'_>, id: Uuid) -> DbResult<()> {
    let r = sqlx::query!(
        "UPDATE jobs SET status = 'queued', attempts = 0, run_at = now(), finished_at = NULL WHERE id = $1 AND status = 'dead'",
        id
    )
    .execute(db)
    .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}
