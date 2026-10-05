//! Event analytics on ClickHouse (module `analytics_clickhouse`).
//!
//! - **Ingestion never slows a request**: [`AnalyticsSink::record`] is a non-blocking
//!   `try_send` into a bounded buffer. A background task batches rows (by size or interval)
//!   into one `INSERT` each, retries a failed batch with backoff, and drops it, counted, if
//!   ClickHouse stays unavailable. When the buffer is full, new events are dropped and counted.
//!   Analytics is lossy by design; anything that must not be lost belongs in PostgreSQL.
//! - **Storage**: `events` (MergeTree, monthly partitions, `ORDER BY (organization_id, event, ts)`,
//!   TTL) and `events_daily` (SummingMergeTree rollup maintained by a materialized view).
//! - **Tenant isolation**: queries take an [`OrgAccess`] proof and always filter by its
//!   organisation; the organisation id never comes from the client.

#[cfg(feature = "clickhouse")]
pub mod schema;

use std::{sync::Arc, time::Duration};

use app_authz::OrgAccess;
#[cfg(feature = "clickhouse")]
use clickhouse::Row;
#[cfg(feature = "clickhouse")]
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
#[cfg(feature = "clickhouse")]
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
#[error("clickhouse: {0}")]
pub struct AnalyticsError(pub String);

#[cfg(feature = "clickhouse")]
impl From<clickhouse::error::Error> for AnalyticsError {
    fn from(e: clickhouse::error::Error) -> Self {
        Self(e.to_string())
    }
}

/// One analytics event as stored.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "clickhouse", derive(Serialize, Deserialize, Row))]
pub struct EventRow {
    #[cfg_attr(feature = "clickhouse", serde(with = "clickhouse::serde::time::datetime64::millis"))]
    pub ts: OffsetDateTime,
    pub event: String,
    #[cfg_attr(feature = "clickhouse", serde(with = "clickhouse::serde::uuid"))]
    pub organization_id: Uuid,
    #[cfg_attr(feature = "clickhouse", serde(with = "clickhouse::serde::uuid::option"))]
    pub user_id: Option<Uuid>,
    pub request_id: String,
    /// JSON object (low-cardinality attributes; never secrets or personal data).
    pub properties: String,
    /// Numeric measure for the event (e.g. calls requested), summed by the daily rollup.
    pub value: f64,
}

impl EventRow {
    pub fn new(event: &str, organization_id: Uuid) -> Self {
        Self {
            ts: OffsetDateTime::now_utc(),
            event: event.to_string(),
            organization_id,
            user_id: None,
            request_id: String::new(),
            properties: "{}".into(),
            value: 0.0,
        }
    }
    pub fn user(mut self, id: Option<Uuid>) -> Self {
        self.user_id = id;
        self
    }
    pub fn request_id(mut self, id: Option<&str>) -> Self {
        self.request_id = id.unwrap_or_default().to_string();
        self
    }
    pub fn value(mut self, v: f64) -> Self {
        self.value = v;
        self
    }
    pub fn properties(mut self, p: &serde_json::Value) -> Self {
        self.properties = p.to_string();
        self
    }
}

/// Where application code sends events. Implementations must never block.
pub trait AnalyticsSink: Send + Sync {
    fn record(&self, event: EventRow);
}

/// Analytics disabled: events are discarded.
pub struct NoopSink;

impl AnalyticsSink for NoopSink {
    fn record(&self, _event: EventRow) {}
}

#[cfg(feature = "clickhouse")]
pub fn client(cfg: &app_config::AnalyticsConfig) -> clickhouse::Client {
    clickhouse::Client::default()
        .with_url(&cfg.clickhouse_url)
        .with_user(&cfg.user)
        .with_password(cfg.password.expose())
        .with_database(&cfg.database)
}

#[derive(Debug, Clone)]
pub struct SinkConfig {
    pub batch_size: usize,
    pub flush_interval: Duration,
    pub buffer_capacity: usize,
    /// Attempts per batch before it is dropped.
    pub insert_attempts: u32,
}

impl From<&app_config::AnalyticsConfig> for SinkConfig {
    fn from(c: &app_config::AnalyticsConfig) -> Self {
        Self {
            batch_size: c.batch_size.max(1),
            flush_interval: Duration::from_millis(c.flush_interval_ms.max(10)),
            buffer_capacity: c.buffer_capacity.max(1),
            insert_attempts: 3,
        }
    }
}

/// Batching ClickHouse sink.
#[cfg(feature = "clickhouse")]
pub struct ClickHouseSink {
    /// The only sender: taking it on shutdown closes the channel, so the batcher drains and ends.
    sender: std::sync::RwLock<Option<mpsc::Sender<EventRow>>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[cfg(feature = "clickhouse")]
impl ClickHouseSink {
    pub fn start(client: clickhouse::Client, cfg: SinkConfig) -> Arc<Self> {
        let (tx, rx) = mpsc::channel(cfg.buffer_capacity);
        let task = tokio::spawn(run_batcher(client, cfg, rx));
        Arc::new(Self { sender: std::sync::RwLock::new(Some(tx)), task: Mutex::new(Some(task)) })
    }

    /// Stop accepting events, flush what is buffered, and wait for the last insert.
    pub async fn shutdown(&self) {
        drop(self.sender.write().unwrap_or_else(std::sync::PoisonError::into_inner).take());
        if let Some(t) = self.task.lock().await.take() {
            let _ = t.await;
        }
    }
}

#[cfg(feature = "clickhouse")]
impl AnalyticsSink for ClickHouseSink {
    fn record(&self, event: EventRow) {
        let guard = self.sender.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(sender) = guard.as_ref() else {
            metrics::counter!("app_analytics_dropped_total", "reason" => "closed").increment(1);
            return;
        };
        match sender.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                metrics::counter!("app_analytics_dropped_total", "reason" => "buffer_full").increment(1);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                metrics::counter!("app_analytics_dropped_total", "reason" => "closed").increment(1);
            }
        }
    }
}

#[cfg(feature = "clickhouse")]
async fn run_batcher(client: clickhouse::Client, cfg: SinkConfig, mut rx: mpsc::Receiver<EventRow>) {
    let mut batch: Vec<EventRow> = Vec::with_capacity(cfg.batch_size);
    loop {
        // Wait for the first row, then fill until the batch is full or the interval elapses.
        let Some(first) = rx.recv().await else { break };
        batch.push(first);
        let deadline = tokio::time::Instant::now() + cfg.flush_interval;
        while batch.len() < cfg.batch_size {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(row)) => batch.push(row),
                Ok(None) | Err(_) => break,
            }
        }
        flush(&client, &cfg, &mut batch).await;
        if rx.is_closed() && rx.is_empty() {
            break;
        }
    }
    if !batch.is_empty() {
        flush(&client, &cfg, &mut batch).await;
    }
}

#[cfg(feature = "clickhouse")]
async fn flush(client: &clickhouse::Client, cfg: &SinkConfig, batch: &mut Vec<EventRow>) {
    if batch.is_empty() {
        return;
    }
    let started = std::time::Instant::now();
    let mut attempt = 0;
    loop {
        attempt += 1;
        match insert_batch(client, batch).await {
            Ok(()) => {
                metrics::counter!("app_analytics_inserted_total").increment(batch.len() as u64);
                metrics::histogram!("app_analytics_insert_seconds").record(started.elapsed().as_secs_f64());
                break;
            }
            Err(e) if attempt < cfg.insert_attempts => {
                tracing::warn!(error = %e, attempt, rows = batch.len(), "analytics insert failed; retrying");
                tokio::time::sleep(Duration::from_millis(200 * 2u64.pow(attempt))).await;
            }
            Err(e) => {
                tracing::error!(error = %e, rows = batch.len(), "analytics batch dropped after retries");
                metrics::counter!("app_analytics_dropped_total", "reason" => "insert_failed")
                    .increment(batch.len() as u64);
                break;
            }
        }
    }
    batch.clear();
}

#[cfg(feature = "clickhouse")]
async fn insert_batch(client: &clickhouse::Client, batch: &[EventRow]) -> Result<(), AnalyticsError> {
    let mut insert = client.insert::<EventRow>("events").await?;
    for row in batch {
        insert.write(row).await?;
    }
    insert.end().await?;
    Ok(())
}

/// One day of an event series for one organisation.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "clickhouse", derive(Serialize, Deserialize, Row))]
pub struct DailyPoint {
    #[cfg_attr(feature = "clickhouse", serde(with = "clickhouse::serde::time::date"))]
    pub day: time::Date,
    pub events: u64,
    pub value: f64,
}

/// Tenant-scoped analytics reads. Only constructible when built with the `clickhouse` feature.
pub struct AnalyticsQuery {
    #[cfg(feature = "clickhouse")]
    client: clickhouse::Client,
    #[cfg(not(feature = "clickhouse"))]
    _never: std::convert::Infallible,
}

#[cfg(not(feature = "clickhouse"))]
impl AnalyticsQuery {
    pub async fn daily(
        &self,
        _access: &OrgAccess,
        _event: &str,
        _days: u32,
    ) -> Result<Vec<DailyPoint>, AnalyticsError> {
        match self._never {}
    }

    pub async fn ping(&self) -> Result<(), AnalyticsError> {
        match self._never {}
    }
}

#[cfg(feature = "clickhouse")]
impl AnalyticsQuery {
    pub fn new(client: clickhouse::Client) -> Self {
        Self { client }
    }

    /// Daily totals of `event` for the caller's organisation over the last `days` days
    /// (from the rollup; `sum` because SummingMergeTree merges asynchronously).
    pub async fn daily(&self, access: &OrgAccess, event: &str, days: u32) -> Result<Vec<DailyPoint>, AnalyticsError> {
        Ok(self
            .client
            .query(
                "SELECT day, sum(events) AS events, sum(value) AS value FROM events_daily \
                 WHERE organization_id = ? AND event = ? AND day >= today() - ? \
                 GROUP BY day ORDER BY day",
            )
            .bind(access.org_id())
            .bind(event)
            .bind(days.min(3650))
            .fetch_all::<DailyPoint>()
            .await?)
    }

    pub async fn ping(&self) -> Result<(), AnalyticsError> {
        self.client.query("SELECT 1").execute().await?;
        Ok(())
    }
}

/// Analytics wiring for a process: the sink to record into, plus handles for queries and
/// shutdown when ClickHouse is enabled.
pub struct Analytics {
    pub sink: Arc<dyn AnalyticsSink>,
    #[cfg(feature = "clickhouse")]
    pub clickhouse: Option<Arc<ClickHouseSink>>,
    pub query: Option<Arc<AnalyticsQuery>>,
}

impl Analytics {
    pub fn disabled() -> Self {
        Self {
            sink: Arc::new(NoopSink),
            #[cfg(feature = "clickhouse")]
            clickhouse: None,
            query: None,
        }
    }

    /// Flush buffered events (call on shutdown).
    pub async fn shutdown(&self) {
        #[cfg(feature = "clickhouse")]
        if let Some(s) = &self.clickhouse {
            s.shutdown().await;
        }
    }
}

/// Start analytics from configuration. ClickHouse being down never prevents startup: the
/// schema migration is retried on the next start and inserts are retried, then dropped.
/// Asking for analytics in a binary built without the `clickhouse` feature is an error.
pub async fn start(cfg: &app_config::AnalyticsConfig) -> Result<Analytics, AnalyticsError> {
    if !cfg.enabled {
        return Ok(Analytics::disabled());
    }
    #[cfg(not(feature = "clickhouse"))]
    return Err(AnalyticsError(
        "analytics.enabled = true but this binary was built without the `clickhouse` feature".into(),
    ));
    #[cfg(feature = "clickhouse")]
    start_clickhouse(cfg).await
}

#[cfg(feature = "clickhouse")]
async fn start_clickhouse(cfg: &app_config::AnalyticsConfig) -> Result<Analytics, AnalyticsError> {
    let ch = client(cfg);
    match schema::migrate(&ch).await {
        Ok(ran) if !ran.is_empty() => tracing::info!(?ran, "analytics schema migrated"),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "analytics schema migration failed (ClickHouse unavailable?)"),
    }
    let sink = ClickHouseSink::start(ch.clone(), SinkConfig::from(cfg));
    tracing::info!(url = %cfg.clickhouse_url, "analytics events to ClickHouse");
    Ok(Analytics { sink: sink.clone(), clickhouse: Some(sink), query: Some(Arc::new(AnalyticsQuery::new(ch))) })
}

/// Test support: create and drop throwaway ClickHouse databases.
#[cfg(feature = "clickhouse")]
pub mod testing {
    pub async fn create_database(
        url: &str,
        user: &str,
        password: &str,
        name: &str,
    ) -> Result<(), super::AnalyticsError> {
        admin(url, user, password).query(&format!("CREATE DATABASE {name}")).execute().await?;
        Ok(())
    }
    pub async fn drop_database(url: &str, user: &str, password: &str, name: &str) {
        let _ = admin(url, user, password).query(&format!("DROP DATABASE IF EXISTS {name}")).execute().await;
    }
    fn admin(url: &str, user: &str, password: &str) -> clickhouse::Client {
        clickhouse::Client::default().with_url(url).with_user(user).with_password(password)
    }
}
