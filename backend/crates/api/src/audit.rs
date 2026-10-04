//! Audit helpers. Security-sensitive changes write their audit row in the *same transaction*
//! as the change (both or neither). Denials and other evidence without a transaction are
//! written best-effort and failures are logged loudly.

use app_db::audit::{AuditEvent, Outcome};
use sqlx::PgExecutor;

use crate::auth::{Principal, ReqMeta};

/// Build an event with actor and request metadata filled in.
pub fn event(
    principal: Option<&Principal>,
    meta: &ReqMeta,
    store_ip: bool,
    action: &'static str,
    outcome: Outcome,
) -> AuditEvent {
    let mut e = AuditEvent::new(action, outcome);
    if let Some(p) = principal {
        e = e.actor(p.kind(), Some(p.id()), Some(p.label()));
    } else {
        e.actor_type = "anonymous";
    }
    e.request_id = meta.request_id.clone();
    e.user_agent = meta.user_agent.clone();
    if store_ip {
        e.ip = meta.ip.map(ipnet::IpNet::from);
    }
    e
}

/// Best-effort insert (for denials and observations outside a transaction).
pub async fn best_effort(db: impl PgExecutor<'_>, e: &AuditEvent) {
    if let Err(err) = app_db::audit::insert(db, e).await {
        metrics::counter!("app_audit_write_failures_total").increment(1);
        tracing::error!(error = %err, action = e.action, "failed to write audit event");
    }
}

/// Trace context stored with a queued job: the W3C context of the current span (so the worker
/// continues the request's trace) plus the request id (log correlation without tracing).
pub fn job_trace_context(request_id: Option<&str>) -> serde_json::Value {
    let mut map: serde_json::Map<String, serde_json::Value> = app_telemetry::propagation::current_context_map()
        .into_iter()
        .map(|(k, v)| (k, serde_json::Value::String(v)))
        .collect();
    if let Some(r) = request_id {
        map.insert("request_id".into(), r.into());
    }
    serde_json::Value::Object(map)
}
