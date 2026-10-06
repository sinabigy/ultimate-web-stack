//! Append-only audit trail. Rows cannot be updated or deleted (database trigger).
//! Metadata must be safe: never passwords, tokens, secrets or raw provider payloads.
//! `AuditEvent::metadata` is filtered through [`scrub`] before insert as a last line of defence.

use ipnet::IpNet;
use sqlx::PgExecutor;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    DbResult,
    pagination::{Cursor, Page, page_from},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Denied,
    Failure,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Success => "success",
            Outcome::Denied => "denied",
            Outcome::Failure => "failure",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub actor_type: &'static str,
    pub actor_id: Option<Uuid>,
    pub actor_label: Option<String>,
    pub action: &'static str,
    pub outcome: Outcome,
    pub target_type: Option<&'static str>,
    pub target_id: Option<String>,
    pub organization_id: Option<Uuid>,
    pub request_id: Option<String>,
    pub ip: Option<IpNet>,
    pub user_agent: Option<String>,
    pub metadata: serde_json::Value,
}

impl AuditEvent {
    pub fn new(action: &'static str, outcome: Outcome) -> Self {
        Self {
            actor_type: "system",
            actor_id: None,
            actor_label: None,
            action,
            outcome,
            target_type: None,
            target_id: None,
            organization_id: None,
            request_id: None,
            ip: None,
            user_agent: None,
            metadata: serde_json::Value::Object(Default::default()),
        }
    }
    pub fn actor(mut self, kind: &'static str, id: Option<Uuid>, label: Option<String>) -> Self {
        self.actor_type = kind;
        self.actor_id = id;
        self.actor_label = label;
        self
    }
    pub fn target(mut self, kind: &'static str, id: impl ToString) -> Self {
        self.target_type = Some(kind);
        self.target_id = Some(id.to_string());
        self
    }
    pub fn org(mut self, id: Uuid) -> Self {
        self.organization_id = Some(id);
        self
    }
    pub fn meta(mut self, v: serde_json::Value) -> Self {
        self.metadata = v;
        self
    }
}

/// Keys whose values are never stored, at any depth (case-insensitive substring match).
const SENSITIVE: &[&str] = &[
    "password",
    "secret",
    "token",
    "authorization",
    "cookie",
    "api_key",
    "apikey",
    "private",
    "credential",
    "otp",
    "code_verifier",
];

/// Replace values of sensitive-looking keys with "[redacted]".
pub fn scrub(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => serde_json::Value::Object(
            m.iter()
                .map(|(k, val)| {
                    let lk = k.to_ascii_lowercase();
                    if SENSITIVE.iter().any(|s| lk.contains(s)) {
                        (k.clone(), serde_json::Value::String("[redacted]".into()))
                    } else {
                        (k.clone(), scrub(val))
                    }
                })
                .collect(),
        ),
        serde_json::Value::Array(a) => serde_json::Value::Array(a.iter().map(scrub).collect()),
        other => other.clone(),
    }
}

pub async fn insert(db: impl PgExecutor<'_>, e: &AuditEvent) -> DbResult<Uuid> {
    let id = app_domain::new_id();
    sqlx::query!(
        r#"INSERT INTO audit_events (id, actor_type, actor_id, actor_label, action, outcome, target_type, target_id,
                                     organization_id, request_id, ip, user_agent, metadata)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)"#,
        id,
        e.actor_type,
        e.actor_id,
        e.actor_label,
        e.action,
        e.outcome.as_str(),
        e.target_type,
        e.target_id,
        e.organization_id,
        e.request_id,
        e.ip,
        e.user_agent,
        scrub(&e.metadata),
    )
    .execute(db)
    .await?;
    Ok(id)
}

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS, utoipa::ToSchema)]
#[ts(export)]
pub struct AuditRow {
    pub id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub occurred_at: OffsetDateTime,
    pub actor_type: String,
    pub actor_id: Option<Uuid>,
    pub actor_label: Option<String>,
    pub action: String,
    pub outcome: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub organization_id: Option<Uuid>,
    pub request_id: Option<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    pub action_prefix: Option<String>,
    pub actor_id: Option<Uuid>,
    pub outcome: Option<String>,
    pub since: Option<OffsetDateTime>,
    pub until: Option<OffsetDateTime>,
}

/// List audit events newest first. `org` = Some(id) scopes to one organisation (callers pass
/// `access.org_id()`); `None` is the global system view (requires `system:audit:read`).
pub async fn list(
    db: impl PgExecutor<'_>,
    org: Option<Uuid>,
    filter: &AuditFilter,
    cursor: Option<Cursor>,
    limit: i64,
) -> DbResult<Page<AuditRow>> {
    let prefix = filter.action_prefix.as_ref().map(|p| format!("{}%", p.replace(['%', '_', '\\'], "")));
    let rows = sqlx::query_as!(
        AuditRow,
        r#"
        SELECT id, occurred_at, actor_type, actor_id, actor_label, action, outcome, target_type, target_id,
               organization_id, request_id, metadata
        FROM audit_events
        WHERE ($1::uuid IS NULL OR organization_id = $1)
          AND ($2::text IS NULL OR action LIKE $2)
          AND ($3::uuid IS NULL OR actor_id = $3)
          AND ($4::text IS NULL OR outcome = $4)
          AND ($5::timestamptz IS NULL OR occurred_at >= $5)
          AND ($6::timestamptz IS NULL OR occurred_at < $6)
          AND ($7::timestamptz IS NULL OR (occurred_at, id) < ($7, $8))
        ORDER BY occurred_at DESC, id DESC
        LIMIT $9
        "#,
        org,
        prefix,
        filter.actor_id,
        filter.outcome,
        filter.since,
        filter.until,
        cursor.map(|c| c.created_at),
        cursor.map(|c| c.id),
        limit + 1
    )
    .fetch_all(db)
    .await?;
    Ok(page_from(rows, limit, |r| Cursor { created_at: r.occurred_at, id: r.id }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scrub_redacts_nested_sensitive_keys() {
        let v = json!({"email": "a@b.c", "access_token": "x", "nested": {"Password": "p", "ok": 1},
                       "list": [{"client_secret": "s"}]});
        let s = scrub(&v);
        assert_eq!(s["email"], "a@b.c");
        assert_eq!(s["access_token"], "[redacted]");
        assert_eq!(s["nested"]["Password"], "[redacted]");
        assert_eq!(s["nested"]["ok"], 1);
        assert_eq!(s["list"][0]["client_secret"], "[redacted]");
    }
}
