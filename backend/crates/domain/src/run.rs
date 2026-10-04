use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ValidationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Queued => "queued",
            RunStatus::Running => "running",
            RunStatus::Completed => "completed",
            RunStatus::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(RunStatus::Queued),
            "running" => Some(RunStatus::Running),
            "completed" => Some(RunStatus::Completed),
            "failed" => Some(RunStatus::Failed),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, RunStatus::Completed | RunStatus::Failed)
    }
}

/// A batch of provider calls executed by a background job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export)]
pub struct Run {
    pub id: Uuid,
    /// Tenant. Every tenant-owned record carries it; queries are always scoped by it.
    pub organization_id: Uuid,
    /// Creator (resource ownership).
    pub owner_id: Uuid,
    pub label: String,
    pub provider: String,
    pub requested: i32,
    pub succeeded: i32,
    pub failed: i32,
    pub status: RunStatus,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub updated_at: OffsetDateTime,
}

/// Validated input for creating a run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export)]
pub struct NewRun {
    pub label: String,
    pub provider: String,
    pub requested: i32,
}

pub const MAX_CALLS_PER_RUN: i32 = 10_000;

impl NewRun {
    pub fn validate(self) -> Result<Self, Vec<ValidationError>> {
        let mut errs = Vec::new();
        let label = self.label.trim().to_string();
        if label.is_empty() || label.chars().count() > 120 {
            errs.push(ValidationError::new("label", "must be 1-120 characters"));
        }
        if !self.provider.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            || self.provider.is_empty()
            || self.provider.len() > 64
        {
            errs.push(ValidationError::new("provider", "must be a provider name"));
        }
        if !(1..=MAX_CALLS_PER_RUN).contains(&self.requested) {
            errs.push(ValidationError::new("requested", format!("must be between 1 and {MAX_CALLS_PER_RUN}")));
        }
        if errs.is_empty() { Ok(Self { label, ..self }) } else { Err(errs) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_run_validation() {
        let ok = NewRun { label: "  smoke  ".into(), provider: "simulated".into(), requested: 10 };
        assert_eq!(ok.validate().expect("valid").label, "smoke");
        let bad = NewRun { label: "".into(), provider: "../etc".into(), requested: 0 };
        assert_eq!(bad.validate().expect_err("invalid").len(), 3);
    }

    #[test]
    fn status_round_trip() {
        for s in [RunStatus::Queued, RunStatus::Running, RunStatus::Completed, RunStatus::Failed] {
            assert_eq!(RunStatus::parse(s.as_str()), Some(s));
        }
        assert!(RunStatus::Failed.is_terminal() && !RunStatus::Running.is_terminal());
    }
}
