//! Browser sessions and in-flight OIDC flows. Tokens are stored only as SHA-256 hashes.

use ipnet::IpNet;
use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{DbError, DbResult};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SessionRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub csrf_token: String,
    pub created_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
    pub rotated_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub idle_expires_at: OffsetDateTime,
    pub auth_time: OffsetDateTime,
    pub amr: Vec<String>,
    pub mfa: bool,
    pub ip: Option<IpNet>,
    pub user_agent: Option<String>,
    pub id_token_enc: Option<Vec<u8>>,
    pub revoked_at: Option<OffsetDateTime>,
    /// True when matched via `previous_token_hash` (grace window after rotation).
    pub matched_previous: bool,
}

pub struct NewSession<'a> {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: &'a [u8],
    pub csrf_token: &'a str,
    pub expires_at: OffsetDateTime,
    pub idle_expires_at: OffsetDateTime,
    pub auth_time: OffsetDateTime,
    pub amr: &'a [String],
    pub mfa: bool,
    pub ip: Option<IpNet>,
    pub user_agent: Option<&'a str>,
    pub id_token_enc: Option<&'a [u8]>,
}

pub async fn create(db: impl PgExecutor<'_>, s: &NewSession<'_>) -> DbResult<()> {
    sqlx::query!(
        r#"INSERT INTO sessions (id, user_id, token_hash, csrf_token, expires_at, idle_expires_at, auth_time, amr, mfa,
                                 ip, user_agent, id_token_enc)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"#,
        s.id,
        s.user_id,
        s.token_hash,
        s.csrf_token,
        s.expires_at,
        s.idle_expires_at,
        s.auth_time,
        s.amr,
        s.mfa,
        s.ip,
        s.user_agent,
        s.id_token_enc,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Find a *usable* session by token hash: not revoked, not past absolute or idle expiry.
/// The previous token (after rotation) is accepted only until `previous_valid_until`.
pub async fn find_active(db: impl PgExecutor<'_>, token_hash: &[u8]) -> DbResult<Option<SessionRow>> {
    Ok(sqlx::query_as!(
        SessionRow,
        r#"
        SELECT id, user_id, csrf_token, created_at, last_seen_at, rotated_at, expires_at, idle_expires_at, auth_time,
               amr, mfa, ip, user_agent, id_token_enc, revoked_at, (token_hash <> $1) AS "matched_previous!"
        FROM sessions
        WHERE (token_hash = $1 OR (previous_token_hash = $1 AND previous_valid_until > now()))
          AND revoked_at IS NULL AND expires_at > now() AND idle_expires_at > now()
        "#,
        token_hash
    )
    .fetch_optional(db)
    .await?)
}

/// Record activity: slide the idle window (never past the absolute expiry).
pub async fn touch(db: impl PgExecutor<'_>, id: Uuid, idle_expires_at: OffsetDateTime) -> DbResult<()> {
    sqlx::query!(
        "UPDATE sessions SET last_seen_at = now(), idle_expires_at = LEAST($2, expires_at) WHERE id = $1 AND revoked_at IS NULL",
        id,
        idle_expires_at
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Replace the session token (and CSRF token). The old token remains valid for `grace`.
/// Returns false when the session was concurrently rotated/revoked (caller keeps old cookie).
pub async fn rotate(
    db: impl PgExecutor<'_>,
    id: Uuid,
    old_hash: &[u8],
    new_hash: &[u8],
    new_csrf: &str,
    grace_until: OffsetDateTime,
) -> DbResult<bool> {
    let r = sqlx::query!(
        r#"UPDATE sessions SET previous_token_hash = token_hash, previous_valid_until = $5,
                  token_hash = $3, csrf_token = $4, rotated_at = now()
           WHERE id = $1 AND token_hash = $2 AND revoked_at IS NULL"#,
        id,
        old_hash,
        new_hash,
        new_csrf,
        grace_until
    )
    .execute(db)
    .await?;
    Ok(r.rows_affected() == 1)
}

/// Upgrade the authentication context after a step-up (re-authentication / MFA).
pub async fn record_reauth(
    db: impl PgExecutor<'_>,
    id: Uuid,
    auth_time: OffsetDateTime,
    amr: &[String],
    mfa: bool,
) -> DbResult<()> {
    sqlx::query!(
        "UPDATE sessions SET auth_time = $2, amr = $3, mfa = $4 WHERE id = $1 AND revoked_at IS NULL",
        id,
        auth_time,
        amr,
        mfa
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn revoke(db: impl PgExecutor<'_>, user_id: Uuid, id: Uuid, reason: &str) -> DbResult<()> {
    let r = sqlx::query!(
        "UPDATE sessions SET revoked_at = now(), revoked_reason = $3 WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
        id,
        user_id,
        reason
    )
    .execute(db)
    .await?;
    if r.rows_affected() == 0 { Err(DbError::NotFound) } else { Ok(()) }
}

/// Revoke all of a user's *active* sessions, optionally keeping one ("sign out other devices").
/// Returns the number of live sessions revoked (already-expired ones are not counted).
pub async fn revoke_all(db: impl PgExecutor<'_>, user_id: Uuid, except: Option<Uuid>, reason: &str) -> DbResult<u64> {
    Ok(sqlx::query!(
        "UPDATE sessions SET revoked_at = now(), revoked_reason = $3
         WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now() AND idle_expires_at > now()
           AND ($2::uuid IS NULL OR id <> $2)",
        user_id,
        except,
        reason
    )
    .execute(db)
    .await?
    .rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionSummary {
    pub id: Uuid,
    pub created_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub mfa: bool,
    pub amr: Vec<String>,
}

pub async fn list_active(db: impl PgExecutor<'_>, user_id: Uuid) -> DbResult<Vec<SessionSummary>> {
    let rows = sqlx::query!(
        r#"SELECT id, created_at, last_seen_at, expires_at, ip, user_agent, mfa, amr FROM sessions
           WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now() AND idle_expires_at > now()
           ORDER BY last_seen_at DESC LIMIT 100"#,
        user_id
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| SessionSummary {
            id: r.id,
            created_at: r.created_at,
            last_seen_at: r.last_seen_at,
            expires_at: r.expires_at,
            ip: r.ip.map(|i| i.addr().to_string()),
            user_agent: r.user_agent,
            mfa: r.mfa,
            amr: r.amr,
        })
        .collect())
}

/// Delete sessions that expired or were revoked more than `keep_days` ago, and stale OIDC flows.
pub async fn purge(db: impl PgExecutor<'_> + Copy, keep_days: i32) -> DbResult<(u64, u64)> {
    let s = sqlx::query!(
        "DELETE FROM sessions WHERE LEAST(expires_at, COALESCE(revoked_at, expires_at)) < now() - make_interval(days => $1)",
        keep_days
    )
    .execute(db)
    .await?
    .rows_affected();
    let f = sqlx::query!("DELETE FROM oidc_flows WHERE expires_at < now()").execute(db).await?.rows_affected();
    Ok((s, f))
}

// ------------------------------------------------------------------ OIDC flows

pub struct OidcFlow {
    pub nonce: String,
    pub pkce_verifier: String,
    pub return_to: String,
    pub intent: String,
}

pub async fn save_flow(
    db: impl PgExecutor<'_>,
    state_hash: &[u8],
    f: &OidcFlow,
    expires_at: OffsetDateTime,
) -> DbResult<()> {
    sqlx::query!(
        "INSERT INTO oidc_flows (state_hash, nonce, pkce_verifier, return_to, intent, expires_at) VALUES ($1, $2, $3, $4, $5, $6)",
        state_hash,
        f.nonce,
        f.pkce_verifier,
        f.return_to,
        f.intent,
        expires_at
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Atomically consume a flow (single use). Expired flows are not returned.
pub async fn take_flow(db: impl PgExecutor<'_>, state_hash: &[u8]) -> DbResult<Option<OidcFlow>> {
    Ok(sqlx::query!(
        "DELETE FROM oidc_flows WHERE state_hash = $1 RETURNING nonce, pkce_verifier, return_to, intent, expires_at",
        state_hash
    )
    .fetch_optional(db)
    .await?
    .filter(|r| r.expires_at > OffsetDateTime::now_utc())
    .map(|r| OidcFlow { nonce: r.nonce, pkce_verifier: r.pkce_verifier, return_to: r.return_to, intent: r.intent }))
}

/// Session plus the user facts needed to authenticate a request, in one indexed query.
#[derive(Debug, Clone)]
pub struct AuthenticatedSession {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub csrf_token: String,
    pub last_seen_at: OffsetDateTime,
    pub rotated_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub auth_time: OffsetDateTime,
    pub amr: Vec<String>,
    pub mfa: bool,
    pub ip: Option<IpNet>,
    pub user_agent: Option<String>,
    pub matched_previous: bool,
    pub email: String,
    pub email_verified: bool,
    pub display_name: String,
    pub user_status: String,
    pub system_role: String,
    pub external_subject: String,
}

pub async fn find_active_with_user(
    db: impl PgExecutor<'_>,
    token_hash: &[u8],
) -> DbResult<Option<AuthenticatedSession>> {
    Ok(sqlx::query_as!(
        AuthenticatedSession,
        r#"
        SELECT s.id AS session_id, s.user_id, s.csrf_token, s.last_seen_at, s.rotated_at, s.expires_at, s.auth_time,
               s.amr, s.mfa, s.ip, s.user_agent, (s.token_hash <> $1) AS "matched_previous!",
               u.email, u.email_verified, u.display_name, u.status AS user_status, u.system_role, u.external_subject
        FROM sessions s JOIN users u ON u.id = s.user_id
        WHERE (s.token_hash = $1 OR (s.previous_token_hash = $1 AND s.previous_valid_until > now()))
          AND s.revoked_at IS NULL AND s.expires_at > now() AND s.idle_expires_at > now()
        "#,
        token_hash
    )
    .fetch_optional(db)
    .await?)
}

/// Encrypted ID token of a session (for RP-initiated logout).
pub async fn id_token_enc(db: impl PgExecutor<'_>, id: Uuid) -> DbResult<Option<Vec<u8>>> {
    Ok(sqlx::query_scalar!("SELECT id_token_enc FROM sessions WHERE id = $1", id).fetch_optional(db).await?.flatten())
}
