use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::run::RunStatus;

/// Events pushed to dashboards over SSE/WebSocket. Tagged JSON: `{"type": "run_progress", ...}`.
/// Payloads carry ids and counters only: never user-supplied content beyond what the
/// recipient could already read, and never provider payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RealtimeEvent {
    RunCreated {
        run_id: Uuid,
        organization_id: Uuid,
        requested: i32,
    },
    RunProgress {
        run_id: Uuid,
        organization_id: Uuid,
        succeeded: i32,
        failed: i32,
        requested: i32,
    },
    RunFinished {
        run_id: Uuid,
        organization_id: Uuid,
        status: RunStatus,
        succeeded: i32,
        failed: i32,
    },
    /// Personal notification for one user.
    Notification {
        user_id: Uuid,
        notification_id: Uuid,
        title: String,
    },
    /// System-wide provider health; delivered only to system administrators.
    ProviderHealth {
        provider: String,
        concurrency_limit: usize,
        inflight: usize,
        healthy: bool,
    },
    Heartbeat {
        at_unix_ms: i64,
    },
}

/// Who may receive an event. Fan-out code must check this against the subscriber's
/// *server-verified* memberships; there is no "broadcast to all" for tenant data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audience {
    Organization(Uuid),
    User(Uuid),
    SystemAdmins,
    Everyone,
}

impl RealtimeEvent {
    pub fn audience(&self) -> Audience {
        match self {
            Self::RunCreated { organization_id, .. }
            | Self::RunProgress { organization_id, .. }
            | Self::RunFinished { organization_id, .. } => Audience::Organization(*organization_id),
            Self::Notification { user_id, .. } => Audience::User(*user_id),
            Self::ProviderHealth { .. } => Audience::SystemAdmins,
            Self::Heartbeat { .. } => Audience::Everyone,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::RunCreated { .. } => "run_created",
            Self::RunProgress { .. } => "run_progress",
            Self::RunFinished { .. } => "run_finished",
            Self::Notification { .. } => "notification",
            Self::ProviderHealth { .. } => "provider_health",
            Self::Heartbeat { .. } => "heartbeat",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagged_json_shape_is_stable() {
        let e = RealtimeEvent::Heartbeat { at_unix_ms: 5 };
        assert_eq!(serde_json::to_string(&e).expect("json"), r#"{"type":"heartbeat","at_unix_ms":5}"#);
        let back: RealtimeEvent = serde_json::from_str(r#"{"type":"heartbeat","at_unix_ms":5}"#).expect("parse");
        assert_eq!(back, e);
        assert_eq!(e.kind(), "heartbeat");
    }
}
