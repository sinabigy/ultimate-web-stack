//! Realtime event fan-out.
//!
//! - Core profile: [`LocalEventBus`], a bounded in-process broadcast. Correct for a single
//!   API instance (or sticky sessions).
//! - Distributed profile: the NATS bus (feature `nats`) publishes to a subject and every
//!   instance re-broadcasts locally, so SSE/WebSocket clients on any instance see events.
//!
//! Delivery is **at-most-once** and lossy under backpressure: slow subscribers skip
//! events (`Lagged`) rather than slowing producers. Anything that must not be lost belongs
//! in PostgreSQL or a JetStream job, with the realtime event as a notification only.

#[cfg(feature = "nats")]
pub mod nats;

use app_domain::RealtimeEvent;
use tokio::sync::broadcast;

#[async_trait::async_trait]
pub trait EventBus: Send + Sync {
    /// Publish to every subscriber in the deployment. Never blocks on slow consumers.
    async fn publish(&self, event: RealtimeEvent);
    /// Subscribe to events delivered to *this* instance.
    fn subscribe(&self) -> broadcast::Receiver<RealtimeEvent>;
}

/// Aborts the task when dropped (background helpers whose owner may be cancelled).
pub struct AbortOnDrop<T>(pub tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub struct LocalEventBus {
    tx: broadcast::Sender<RealtimeEvent>,
}

impl LocalEventBus {
    pub fn new(capacity: usize) -> Self {
        Self { tx: broadcast::channel(capacity).0 }
    }

    /// Deliver to local subscribers only (used by distributed buses on receipt).
    pub fn deliver_local(&self, event: RealtimeEvent) {
        metrics::counter!("app_events_published_total", "kind" => event.kind()).increment(1);
        // Err means "no subscribers", which is fine.
        let _ = self.tx.send(event);
    }

    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for LocalEventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}

#[async_trait::async_trait]
impl EventBus for LocalEventBus {
    async fn publish(&self, event: RealtimeEvent) {
        self.deliver_local(event);
    }

    fn subscribe(&self) -> broadcast::Receiver<RealtimeEvent> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fan_out_and_lag_instead_of_blocking() {
        let bus = LocalEventBus::new(2);
        let mut a = bus.subscribe();
        for i in 0..5 {
            bus.publish(RealtimeEvent::Heartbeat { at_unix_ms: i }).await;
        }
        // producer was never blocked; the slow subscriber observes lag then the newest events
        assert!(matches!(a.recv().await, Err(broadcast::error::RecvError::Lagged(3))));
        assert_eq!(a.recv().await.ok(), Some(RealtimeEvent::Heartbeat { at_unix_ms: 3 }));
    }
}
