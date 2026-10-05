//! PostgreSQL-backed event bus (core profile, multiple processes).
//!
//! `publish` sends `pg_notify('app_events', json)`; every process LISTENs and re-delivers to its
//! local subscribers. This lets a separate worker process push run progress to SSE clients
//! connected to any API instance without adding NATS. Payloads are small (ids + counters) and
//! well under PostgreSQL's 8000-byte NOTIFY limit. Like the local bus it is at-most-once.

use std::sync::Arc;

use app_domain::RealtimeEvent;
use app_messaging::{EventBus, LocalEventBus};
use sqlx::{PgPool, postgres::PgListener};
use tokio::sync::broadcast;

pub struct PgEventBus {
    db: PgPool,
    local: Arc<LocalEventBus>,
    listener: tokio::task::JoinHandle<()>,
}

impl Drop for PgEventBus {
    fn drop(&mut self) {
        // Release the dedicated LISTEN connection when the bus goes away.
        self.listener.abort();
    }
}

impl PgEventBus {
    /// Start the listener task and return the bus.
    pub async fn start(db: PgPool) -> Result<Arc<Self>, sqlx::Error> {
        let local = Arc::new(LocalEventBus::new(4096));
        let mut listener = PgListener::connect_with(&db).await?;
        listener.listen("app_events").await?;
        let deliver = local.clone();
        let listener = tokio::spawn(async move {
            loop {
                match listener.recv().await {
                    Ok(n) => match serde_json::from_str::<RealtimeEvent>(n.payload()) {
                        Ok(ev) => deliver.deliver_local(ev),
                        Err(e) => tracing::warn!(error = %e, "ignoring malformed event"),
                    },
                    // PgListener reconnects automatically on the next recv; back off briefly.
                    Err(e) => {
                        tracing::warn!(error = %e, "event listener error");
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                }
            }
        });
        Ok(Arc::new(Self { db, local, listener }))
    }
}

#[async_trait::async_trait]
impl EventBus for PgEventBus {
    async fn publish(&self, event: RealtimeEvent) {
        let Ok(payload) = serde_json::to_string(&event) else { return };
        if let Err(e) = sqlx::query("SELECT pg_notify('app_events', $1)").bind(payload).execute(&self.db).await {
            // Realtime is best effort: fall back to local delivery so this process still sees it.
            tracing::warn!(error = %e, "pg_notify failed; delivering locally only");
            self.local.deliver_local(event);
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<RealtimeEvent> {
        self.local.subscribe()
    }
}

/// The deployment's realtime bus: NATS when `messaging.enabled` (scale-out, no database load),
/// otherwise PostgreSQL LISTEN/NOTIFY (no extra infrastructure). Both reach every instance.
pub async fn start_event_bus(
    cfg: &app_config::MessagingConfig,
    pool: sqlx::PgPool,
    client_name: &str,
) -> Result<std::sync::Arc<dyn app_messaging::EventBus>, String> {
    if cfg.enabled {
        #[cfg(not(feature = "nats"))]
        let _ = client_name;
        #[cfg(not(feature = "nats"))]
        return Err("messaging.enabled = true but this binary was built without the `nats` feature".into());
        #[cfg(feature = "nats")]
        {
            let started = match app_messaging::nats::connect(&cfg.nats_url, client_name).await {
                Ok(client) => app_messaging::nats::NatsEventBus::start(client, &cfg.events_subject).await,
                Err(e) => Err(e),
            };
            match started {
                Ok(bus) => {
                    tracing::info!(url = %cfg.nats_url, subject = %cfg.events_subject, "realtime events over NATS");
                    return Ok(bus);
                }
                // Realtime fan-out is not critical: run on PostgreSQL rather than refuse to start.
                Err(e) => {
                    metrics::counter!("app_event_bus_fallbacks_total").increment(1);
                    tracing::warn!(error = %e, url = %cfg.nats_url, "NATS unavailable; realtime events fall back to PostgreSQL");
                }
            }
        }
    }
    Ok(PgEventBus::start(pool).await.map_err(|e| e.to_string())?)
}
