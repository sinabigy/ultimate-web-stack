//! NATS adapters (feature `nats`): a distributed realtime [`EventBus`] and a JetStream durable
//! work queue with retries, a dead-letter stream and publish-side deduplication.
//!
//! Delivery guarantees of the queue: **at least once**. A message is removed only after the
//! handler succeeds (explicit ack) or after it is copied to the dead-letter stream. Publishing
//! with an idempotency key is deduplicated by JetStream within `duplicate_window`; handlers must
//! still be idempotent (redelivery after a crash between side effect and ack is possible).

use std::{collections::HashMap, sync::Arc, time::Duration};

use app_domain::RealtimeEvent;
use async_nats::{
    HeaderMap,
    jetstream::{
        self, AckKind,
        consumer::{AckPolicy, pull},
        message::PublishMessage as Publish,
        stream::{self, RetentionPolicy, StorageType},
    },
};
use bytes::Bytes;
use futures::StreamExt as _;
use tokio::{
    sync::{Semaphore, broadcast},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

use crate::{EventBus, LocalEventBus};

#[derive(Debug, thiserror::Error)]
#[error("nats: {0}")]
pub struct NatsError(pub String);

fn err<E: std::fmt::Display>(e: E) -> NatsError {
    NatsError(e.to_string())
}

/// Connect with a bounded timeout (startup fails fast instead of hanging on a dead server;
/// after connecting, the client reconnects automatically).
pub async fn connect(url: &str, name: &str) -> Result<async_nats::Client, NatsError> {
    async_nats::ConnectOptions::new()
        .name(name)
        .connection_timeout(Duration::from_secs(5))
        .connect(url)
        .await
        .map_err(err)
}

// ── realtime event bus ──────────────────────────────────────────────────────────────────────

/// Publishes events to a NATS subject; every instance subscribes and re-broadcasts locally,
/// so SSE/WebSocket clients connected to any instance receive every event. At-most-once:
/// if NATS is unreachable the event is still delivered to this instance's subscribers.
pub struct NatsEventBus {
    client: async_nats::Client,
    subject: String,
    local: Arc<LocalEventBus>,
    task: tokio::task::JoinHandle<()>,
}

impl NatsEventBus {
    pub async fn start(client: async_nats::Client, subject: &str) -> Result<Arc<Self>, NatsError> {
        let local = Arc::new(LocalEventBus::default());
        let mut sub = client.subscribe(subject.to_string()).await.map_err(err)?;
        let sink = local.clone();
        let task = tokio::spawn(async move {
            while let Some(m) = sub.next().await {
                match serde_json::from_slice::<RealtimeEvent>(&m.payload) {
                    Ok(ev) => sink.deliver_local(ev),
                    Err(e) => tracing::warn!(error = %e, "undecodable realtime event ignored"),
                }
            }
        });
        Ok(Arc::new(Self { client, subject: subject.to_string(), local, task }))
    }
}

impl Drop for NatsEventBus {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[async_trait::async_trait]
impl EventBus for NatsEventBus {
    async fn publish(&self, event: RealtimeEvent) {
        let sent = match serde_json::to_vec(&event) {
            Ok(body) => self.client.publish(self.subject.clone(), body.into()).await.map_err(err),
            Err(e) => Err(err(e)),
        };
        if let Err(e) = sent {
            // Our own subscription would have delivered it locally; do that directly.
            metrics::counter!("app_events_publish_failures_total", "bus" => "nats").increment(1);
            tracing::warn!(error = %e, "NATS publish failed; delivering locally only");
            self.local.deliver_local(event);
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<RealtimeEvent> {
        self.local.subscribe()
    }
    fn transport(&self) -> &'static str {
        "nats"
    }
    async fn health(&self) -> Result<(), String> {
        match self.client.connection_state() {
            async_nats::connection::State::Connected => Ok(()),
            other => Err(format!("NATS {other}")),
        }
    }
}

// ── JetStream work queue ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct QueueConfig {
    /// Stream name, e.g. `APP_JOBS`. The dead-letter stream is `<stream>_DLQ`.
    pub stream: String,
    /// Subject prefix: messages are published to `<prefix>.<kind>`.
    pub subject_prefix: String,
    /// Deliveries before a failing message is dead-lettered.
    pub max_deliver: i64,
    /// Redelivery after this long without ack (handlers send progress for long work).
    pub ack_wait: Duration,
    /// Delay before retry n (the last entry repeats).
    pub backoff: Vec<Duration>,
    /// Publish-side deduplication window for idempotency keys.
    pub duplicate_window: Duration,
    pub dlq_max_age: Duration,
    pub replicas: usize,
}

impl QueueConfig {
    pub fn new(stream: &str) -> Self {
        Self {
            stream: stream.to_string(),
            subject_prefix: stream.to_ascii_lowercase(),
            max_deliver: 5,
            ack_wait: Duration::from_secs(30),
            backoff: vec![
                Duration::from_secs(1),
                Duration::from_secs(5),
                Duration::from_secs(30),
                Duration::from_secs(120),
            ],
            duplicate_window: Duration::from_secs(120),
            dlq_max_age: Duration::from_secs(14 * 24 * 3600),
            replicas: 1,
        }
    }

    fn dlq_stream(&self) -> String {
        format!("{}_DLQ", self.stream)
    }

    fn dlq_subject(&self, original: &str) -> String {
        format!("dlq.{original}")
    }

    fn retry_delay(&self, attempt: i64) -> Duration {
        let i = usize::try_from(attempt.saturating_sub(1)).unwrap_or(0);
        self.backoff.get(i).or(self.backoff.last()).copied().unwrap_or(Duration::from_secs(1))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Published {
    pub sequence: u64,
    /// The idempotency key was seen within the duplicate window: nothing new was stored.
    pub duplicate: bool,
}

/// A dead-lettered message, with why and after how many attempts.
#[derive(Debug, Clone)]
pub struct DeadLetter {
    pub dlq_sequence: u64,
    pub original_subject: String,
    pub reason: String,
    pub attempts: i64,
    pub payload: Bytes,
}

pub struct JetStreamQueue {
    js: jetstream::Context,
    client: async_nats::Client,
    cfg: QueueConfig,
}

impl JetStreamQueue {
    /// Ensure the work-queue stream and its dead-letter stream exist.
    pub async fn new(client: async_nats::Client, cfg: QueueConfig) -> Result<Arc<Self>, NatsError> {
        let js = jetstream::new(client.clone());
        js.get_or_create_stream(stream::Config {
            name: cfg.stream.clone(),
            subjects: vec![format!("{}.>", cfg.subject_prefix)],
            // Work queue: each message is consumed by one consumer and removed when acked.
            retention: RetentionPolicy::WorkQueue,
            storage: StorageType::File,
            duplicate_window: cfg.duplicate_window,
            num_replicas: cfg.replicas,
            ..Default::default()
        })
        .await
        .map_err(err)?;
        js.get_or_create_stream(stream::Config {
            name: cfg.dlq_stream(),
            subjects: vec![format!("dlq.{}.>", cfg.subject_prefix)],
            retention: RetentionPolicy::Limits,
            storage: StorageType::File,
            max_age: cfg.dlq_max_age,
            duplicate_window: cfg.duplicate_window,
            num_replicas: cfg.replicas,
            ..Default::default()
        })
        .await
        .map_err(err)?;
        Ok(Arc::new(Self { js, client, cfg }))
    }

    pub fn config(&self) -> &QueueConfig {
        &self.cfg
    }

    pub fn subject(&self, kind: &str) -> String {
        format!("{}.{kind}", self.cfg.subject_prefix)
    }

    /// Publish a job. The current trace context travels in the headers; an idempotency key
    /// becomes `Nats-Msg-Id` (duplicates within the window are acknowledged but not stored).
    pub async fn publish(
        &self,
        kind: &str,
        payload: Bytes,
        idempotency_key: Option<&str>,
    ) -> Result<Published, NatsError> {
        if kind.is_empty() || !kind.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
            return Err(NatsError(format!("invalid job kind {kind:?}")));
        }
        let mut headers = HeaderMap::new();
        for (k, v) in app_telemetry::propagation::current_context_map() {
            headers.insert(k.as_str(), v.as_str());
        }
        let mut msg = Publish::build().payload(payload).headers(headers);
        if let Some(key) = idempotency_key {
            msg = msg.message_id(key);
        }
        let ack = self.js.send_publish(self.subject(kind), msg).await.map_err(err)?.await.map_err(err)?;
        let result = if ack.duplicate { "duplicate" } else { "stored" };
        metrics::counter!("app_queue_published_total", "stream" => self.cfg.stream.clone(), "result" => result)
            .increment(1);
        Ok(Published { sequence: ack.sequence, duplicate: ack.duplicate })
    }

    /// Messages currently waiting in the work queue (not yet acked).
    pub async fn pending(&self) -> Result<u64, NatsError> {
        let mut s = self.js.get_stream(&self.cfg.stream).await.map_err(err)?;
        Ok(s.info().await.map_err(err)?.state.messages)
    }

    async fn dead_letter(
        &self,
        subject: &str,
        headers: Option<&HeaderMap>,
        payload: Bytes,
        reason: &str,
        attempts: i64,
        stream_seq: u64,
    ) -> Result<(), NatsError> {
        let mut h = headers.cloned().unwrap_or_default();
        h.insert("Dlq-Reason", reason.chars().take(512).collect::<String>().as_str());
        h.insert("Dlq-Original-Subject", subject);
        h.insert("Dlq-Attempts", attempts.to_string().as_str());
        h.insert("Dlq-Stream-Sequence", stream_seq.to_string().as_str());
        // Deterministic id: the worker and the max-deliveries sweeper may both try.
        let msg =
            Publish::build().payload(payload).headers(h).message_id(format!("dlq:{}:{stream_seq}", self.cfg.stream));
        self.js.send_publish(self.cfg.dlq_subject(subject), msg).await.map_err(err)?.await.map_err(err)?;
        metrics::counter!("app_queue_dead_lettered_total", "stream" => self.cfg.stream.clone()).increment(1);
        tracing::warn!(subject, attempts, reason, "message dead-lettered");
        Ok(())
    }

    /// Read up to `max` dead letters (oldest first) without removing them.
    pub async fn dead_letters(&self, max: usize) -> Result<Vec<DeadLetter>, NatsError> {
        let mut s = self.js.get_stream(self.cfg.dlq_stream()).await.map_err(err)?;
        let state = s.info().await.map_err(err)?.state.clone();
        let mut out = Vec::new();
        if state.messages == 0 {
            return Ok(out);
        }
        for seq in state.first_sequence..=state.last_sequence {
            if out.len() >= max {
                break;
            }
            let Ok(m) = s.get_raw_message(seq).await else { continue }; // deleted gap
            let h = |k: &str| m.headers.get(k).map(|v| v.as_str().to_string()).unwrap_or_default();
            out.push(DeadLetter {
                dlq_sequence: m.sequence,
                original_subject: h("Dlq-Original-Subject"),
                reason: h("Dlq-Reason"),
                attempts: h("Dlq-Attempts").parse().unwrap_or(0),
                payload: m.payload.clone(),
            });
        }
        Ok(out)
    }

    /// Re-publish dead letters to their original subject (fresh attempts) and remove them
    /// from the dead-letter stream. Returns how many were replayed.
    pub async fn replay_dead_letters(&self, max: usize) -> Result<usize, NatsError> {
        let dlq = self.js.get_stream(self.cfg.dlq_stream()).await.map_err(err)?;
        let mut n = 0;
        for d in self.dead_letters(max).await? {
            let msg = Publish::build()
                .payload(d.payload)
                .message_id(format!("replay:{}:{}", self.cfg.stream, d.dlq_sequence));
            self.js.send_publish(d.original_subject.clone(), msg).await.map_err(err)?.await.map_err(err)?;
            dlq.delete_message(d.dlq_sequence).await.map_err(err)?;
            n += 1;
        }
        Ok(n)
    }

    /// Dead-letter messages whose *last* delivery was never acknowledged (the worker crashed or
    /// timed out), which JetStream would otherwise leave stranded in the work queue. Listens for
    /// the server's MAX_DELIVERIES advisory for this consumer.
    pub async fn run_max_deliveries_sweeper(
        self: Arc<Self>,
        durable: String,
        shutdown: CancellationToken,
    ) -> Result<(), NatsError> {
        #[derive(serde::Deserialize)]
        struct Advisory {
            stream_seq: u64,
            deliveries: i64,
        }
        let subject = format!("$JS.EVENT.ADVISORY.CONSUMER.MAX_DELIVERIES.{}.{durable}", self.cfg.stream);
        let mut sub = self.client.subscribe(subject).await.map_err(err)?;
        let stream = self.js.get_stream(&self.cfg.stream).await.map_err(err)?;
        loop {
            let m = tokio::select! {
                _ = shutdown.cancelled() => return Ok(()),
                m = sub.next() => match m { Some(m) => m, None => return Ok(()) },
            };
            let Ok(a) = serde_json::from_slice::<Advisory>(&m.payload) else { continue };
            // Already acked/terminated (e.g. dead-lettered by the worker itself): nothing to do.
            let Ok(raw) = stream.get_raw_message(a.stream_seq).await else { continue };
            let headers = raw.headers.clone();
            if let Err(e) = self
                .dead_letter(
                    raw.subject.as_str(),
                    Some(&headers),
                    raw.payload.clone(),
                    "max deliveries reached without acknowledgement (worker crashed or exceeded ack_wait)",
                    a.deliveries,
                    a.stream_seq,
                )
                .await
            {
                tracing::error!(error = %e, seq = a.stream_seq, "could not dead-letter stranded message");
                continue;
            }
            if let Err(e) = stream.delete_message(a.stream_seq).await {
                tracing::warn!(error = %e, seq = a.stream_seq, "could not delete stranded message after dead-lettering");
            }
        }
    }
}

/// One delivery of a queued message.
#[derive(Debug)]
pub struct Delivery<'a> {
    pub kind: &'a str,
    pub subject: &'a str,
    pub payload: &'a [u8],
    /// 1 for the first delivery.
    pub attempt: i64,
    pub max_attempts: i64,
    pub idempotency_key: Option<&'a str>,
    pub stream_sequence: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    /// Try again later (with backoff); dead-lettered once attempts are exhausted.
    #[error("retryable: {0}")]
    Retry(String),
    /// Will never succeed (invalid payload, missing entity): dead-letter now.
    #[error("permanent: {0}")]
    Permanent(String),
}

#[async_trait::async_trait]
pub trait MessageHandler: Send + Sync {
    async fn handle(&self, delivery: &Delivery<'_>) -> Result<(), HandlerError>;
}

pub struct JetStreamWorker {
    pub queue: Arc<JetStreamQueue>,
    /// Durable consumer name (one per logical worker group).
    pub durable: String,
    /// Subject filter, e.g. `app_jobs.>` (all kinds) or `app_jobs.email`.
    pub filter: String,
    pub handler: Arc<dyn MessageHandler>,
    pub concurrency: usize,
    /// How long in-flight handlers get after shutdown starts (unacked work is redelivered).
    pub shutdown_grace: Duration,
}

impl JetStreamWorker {
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), NatsError> {
        let cfg = self.queue.cfg.clone();
        let concurrency = self.concurrency.max(1);
        let stream = self.queue.js.get_stream(&cfg.stream).await.map_err(err)?;
        let consumer: jetstream::consumer::Consumer<pull::Config> = stream
            .get_or_create_consumer(
                &self.durable,
                pull::Config {
                    durable_name: Some(self.durable.clone()),
                    ack_policy: AckPolicy::Explicit,
                    ack_wait: cfg.ack_wait,
                    max_deliver: cfg.max_deliver,
                    // Bounded: messages waiting in this client's buffer also run their ack timer.
                    max_ack_pending: i64::try_from(concurrency * 2).unwrap_or(i64::MAX),
                    filter_subject: self.filter.clone(),
                    ..Default::default()
                },
            )
            .await
            .map_err(err)?;
        let sweeper =
            tokio::spawn(self.queue.clone().run_max_deliveries_sweeper(self.durable.clone(), shutdown.clone()));
        let mut messages = consumer
            .stream()
            .max_messages_per_batch(concurrency)
            .expires(Duration::from_secs(5))
            .heartbeat(Duration::from_secs(2))
            .messages()
            .await
            .map_err(err)?;
        let permits = Arc::new(Semaphore::new(concurrency));
        let mut tasks = JoinSet::new();
        loop {
            let permit = tokio::select! {
                _ = shutdown.cancelled() => break,
                p = permits.clone().acquire_owned() => match p { Ok(p) => p, Err(_) => break },
            };
            let next = tokio::select! {
                _ = shutdown.cancelled() => break,
                n = messages.next() => n,
                Some(_) = tasks.join_next(), if !tasks.is_empty() => { continue; }
            };
            match next {
                Some(Ok(msg)) => {
                    let (queue, handler) = (self.queue.clone(), self.handler.clone());
                    tasks.spawn(async move {
                        process(&queue, handler.as_ref(), msg).await;
                        drop(permit);
                    });
                }
                Some(Err(e)) => {
                    tracing::warn!(error = %e, "JetStream pull error; retrying");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                None => break,
            }
            while tasks.try_join_next().is_some() {}
        }
        // Stop pulling; unprocessed buffered messages are redelivered after ack_wait.
        drop(messages);
        let drain = async { while tasks.join_next().await.is_some() {} };
        if tokio::time::timeout(self.shutdown_grace, drain).await.is_err() {
            tracing::warn!("shutdown grace elapsed; unacknowledged messages will be redelivered");
        }
        sweeper.abort();
        Ok(())
    }
}

async fn process(queue: &JetStreamQueue, handler: &dyn MessageHandler, msg: jetstream::Message) {
    let cfg = &queue.cfg;
    let (attempt, seq) = match msg.info() {
        Ok(i) => (i.delivered, i.stream_sequence),
        Err(e) => {
            tracing::error!(error = %e, "message without JetStream metadata; terminating");
            let _ = msg.ack_with(AckKind::Term).await;
            return;
        }
    };
    let subject = msg.subject.to_string();
    let kind = subject.strip_prefix(&format!("{}.", cfg.subject_prefix)).unwrap_or(&subject).to_string();
    let span = tracing::info_span!("message", stream = %cfg.stream, kind = %kind, attempt, seq);
    if let Some(h) = &msg.headers {
        let map: HashMap<String, String> =
            h.iter().filter_map(|(k, v)| v.first().map(|v| (k.to_string(), v.as_str().to_string()))).collect();
        app_telemetry::propagation::set_parent_from_map(&span, &map);
    }
    async {
        // Long handlers keep their lease: progress acks reset the ack timer. Aborted on drop,
        // so a cancelled handler (crash, abort) stops extending the lease and gets redelivered.
        let progress = {
            let m = msg.clone();
            let every = cfg.ack_wait / 3;
            crate::AbortOnDrop(tokio::spawn(async move {
                let mut t = tokio::time::interval(every.max(Duration::from_millis(100)));
                t.tick().await;
                loop {
                    t.tick().await;
                    let _ = m.ack_with(AckKind::Progress).await;
                }
            }))
        };
        let idempotency_key =
            msg.headers.as_ref().and_then(|h| h.get(async_nats::header::NATS_MESSAGE_ID)).map(|v| v.as_str().to_string());
        let delivery = Delivery {
            kind: &kind,
            subject: &subject,
            payload: &msg.payload,
            attempt,
            max_attempts: cfg.max_deliver,
            idempotency_key: idempotency_key.as_deref(),
            stream_sequence: seq,
        };
        let started = std::time::Instant::now();
        let result = handler.handle(&delivery).await;
        drop(progress);
        let label = match result {
            Ok(()) => {
                // Double ack: wait for the server to confirm, which narrows the window for
                // duplicate redelivery.
                if let Err(e) = msg.double_ack().await {
                    tracing::warn!(error = %e, "ack not confirmed; message may be redelivered");
                }
                "succeeded"
            }
            Err(HandlerError::Retry(reason)) if attempt < cfg.max_deliver => {
                let delay = cfg.retry_delay(attempt);
                tracing::warn!(reason, attempt, ?delay, "message failed; will retry");
                let _ = msg.ack_with(AckKind::Nak(Some(delay))).await;
                "retry"
            }
            Err(e) => {
                let reason = e.to_string();
                match queue.dead_letter(&subject, msg.headers.as_ref(), msg.payload.clone(), &reason, attempt, seq).await {
                    Ok(()) => {
                        let _ = msg.ack_with(AckKind::Term).await;
                        "dead"
                    }
                    Err(dlq_err) => {
                        // Never drop a message we could not preserve: let it be redelivered.
                        tracing::error!(error = %dlq_err, "dead-letter publish failed; message will be redelivered");
                        let _ = msg.ack_with(AckKind::Nak(Some(cfg.retry_delay(attempt)))).await;
                        "error"
                    }
                }
            }
        };
        metrics::counter!("app_queue_processed_total", "stream" => cfg.stream.clone(), "kind" => kind.clone(), "result" => label)
            .increment(1);
        metrics::histogram!("app_queue_handler_seconds", "kind" => kind.clone()).record(started.elapsed().as_secs_f64());
    }
    .instrument(span)
    .await;
}
