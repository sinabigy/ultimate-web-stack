//! Background jobs.
//!
//! [`runtime::PgWorker`] processes the PostgreSQL job queue (core profile): claims with
//! `FOR UPDATE SKIP LOCKED`, leases with heartbeats, LISTEN/NOTIFY wake-ups with polling
//! fallback, reaping of expired leases, retry with backoff and dead-lettering, and graceful
//! shutdown. Delivery is at-least-once: every handler must be idempotent.
//!
//! The messaging profile replaces the transport with JetStream (`app-messaging`, feature
//! `nats`) behind the same [`JobHandler`] trait.

pub mod events;
pub mod handlers;
pub mod runtime;

use std::sync::Arc;

use app_messaging::EventBus;
use app_networking::ProviderRegistry;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

pub use runtime::{PgWorker, WorkerConfig};

/// Why a job attempt failed.
#[derive(Debug, thiserror::Error)]
pub enum JobError {
    /// Try again later (with backoff), e.g. provider unavailable or interrupted by shutdown.
    #[error("retryable: {0}")]
    Retryable(String),
    /// Will never succeed (bad payload, deleted resource): dead-letter immediately.
    #[error("permanent: {0}")]
    Permanent(String),
}

/// Shared services available to handlers.
#[derive(Clone)]
pub struct JobServices {
    pub db: PgPool,
    pub events: Arc<dyn EventBus>,
    pub providers: ProviderRegistry,
    /// Analytics events (ClickHouse when enabled; `NoopSink` otherwise). Never blocks.
    pub analytics: Arc<dyn app_analytics::AnalyticsSink>,
}

pub struct JobContext<'a> {
    pub services: &'a JobServices,
    pub job: &'a app_db::jobs::JobRow,
    /// Cancelled on shutdown: long handlers should checkpoint and return `Retryable`.
    pub shutdown: CancellationToken,
}

#[async_trait::async_trait]
pub trait JobHandler: Send + Sync {
    fn kind(&self) -> &'static str;
    async fn handle(&self, ctx: &JobContext<'_>) -> Result<(), JobError>;
}
