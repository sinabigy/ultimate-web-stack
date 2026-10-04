//! Domain model: users, organisations (tenants) and, for the example application, an
//! operations console where organisation members launch **runs**, each a batch of calls to an external provider executed by a
//! background job, with progress streamed live to the dashboard.
//!
//! This crate is deliberately free of IO and framework types so it can be reused by the
//! API, workers and any future extracted service.

pub mod event;
pub mod org;
pub mod run;
pub mod user;

pub use event::RealtimeEvent;
pub use org::{Organization, Slug};
pub use run::{NewRun, Run, RunStatus};
pub use user::{Email, OrgRole, SystemRole, User, UserStatus};

/// A validation failure on a single input field.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {message}")]
pub struct ValidationError {
    pub field: &'static str,
    pub message: String,
}

impl ValidationError {
    pub fn new(field: &'static str, message: impl Into<String>) -> Self {
        Self { field, message: message.into() }
    }
}

/// Time-ordered UUIDv7: sortable, index-friendly (append-mostly B-tree inserts) and safe to
/// expose. Generated in the application so ids are known before the INSERT.
pub fn new_id() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}
