//! PostgreSQL: the transactional source of truth.
//!
//! - All queries are SQLx macros checked at compile time against the schema (offline data
//!   in `backend/.sqlx/`; regenerate with `./dev db prepare`).
//! - Tenant data is reachable only through functions taking `&OrgAccess`; the organisation
//!   id is taken from that proof, never from request input.
//! - Functions take `impl PgExecutor` (pool or transaction) unless they must run several
//!   statements atomically, in which case they take `&mut PgConnection` and the caller owns
//!   the transaction.

pub mod api_keys;
pub mod audit;
pub mod error;
pub mod jobs;
pub mod notifications;
pub mod orgs;
pub mod pagination;
pub mod runs;
pub mod sessions;
pub mod users;

use std::time::Duration;

use app_config::DatabaseConfig;
pub use error::{DbError, DbResult};
use sqlx::{
    ConnectOptions, PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
pub use sqlx::{PgConnection, Postgres, Transaction};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// Build the connection pool. Every connection gets a server-side `statement_timeout`
/// and an `application_name` (visible in `pg_stat_activity`), and slow statements are logged.
pub async fn connect(cfg: &DatabaseConfig, application_name: &str) -> DbResult<PgPool> {
    let opts: PgConnectOptions = cfg
        .url
        .expose()
        .parse::<PgConnectOptions>()
        .map_err(DbError::Other)?
        .application_name(application_name)
        // Startup parameter: applied by the server to every connection, no extra round trip.
        .options([("statement_timeout", cfg.statement_timeout_ms.to_string())])
        .log_statements(tracing::log::LevelFilter::Trace)
        .log_slow_statements(tracing::log::LevelFilter::Warn, Duration::from_millis(cfg.slow_statement_ms));
    let pool = PgPoolOptions::new()
        .max_connections(cfg.max_connections)
        .min_connections(cfg.min_connections)
        .acquire_timeout(Duration::from_millis(cfg.acquire_timeout_ms))
        .idle_timeout(Some(Duration::from_millis(cfg.idle_timeout_ms)))
        .test_before_acquire(false)
        .connect_lazy_with(opts);
    Ok(pool)
}

/// Apply pending migrations (idempotent).
pub async fn migrate(pool: &PgPool) -> DbResult<()> {
    MIGRATOR.run(pool).await.map_err(|e| DbError::Other(e.into()))
}

/// Liveness of the database for `/readyz`.
pub async fn ping(pool: &PgPool) -> DbResult<()> {
    sqlx::query_scalar!("SELECT 1 AS \"one!\"").fetch_one(pool).await?;
    Ok(())
}

/// Pool saturation snapshot for metrics and `/admin/system`.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct PoolStats {
    pub size: u32,
    pub idle: usize,
    pub max: u32,
}

pub fn pool_stats(pool: &PgPool) -> PoolStats {
    PoolStats { size: pool.size(), idle: pool.num_idle(), max: pool.options().get_max_connections() }
}
