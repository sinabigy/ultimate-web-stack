#![allow(clippy::unwrap_used)]
//! NATS / JetStream integration. Runs when TEST_NATS_URL is set, e.g.
//! TEST_NATS_URL=nats://127.0.0.1:54222 (`./dev up --with messaging_nats` starts NATS).
//! Every test uses its own streams and deletes them afterwards.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use app_domain::RealtimeEvent;
use app_messaging::{
    EventBus,
    nats::{
        Delivery, HandlerError, JetStreamQueue, JetStreamWorker, MessageHandler, NatsEventBus, QueueConfig, connect,
    },
};
use bytes::Bytes;
use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

fn url() -> Option<String> {
    match std::env::var("TEST_NATS_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            eprintln!("TEST_NATS_URL not set: skipping NATS integration test");
            None
        }
    }
}

fn unique(prefix: &str) -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    format!("{prefix}{:X}{}", t % 0xFFFF_FFFF, N.fetch_add(1, Ordering::Relaxed))
}

fn fast_config(stream: &str) -> QueueConfig {
    QueueConfig {
        max_deliver: 3,
        ack_wait: Duration::from_millis(800),
        backoff: vec![Duration::from_millis(50)],
        ..QueueConfig::new(stream)
    }
}

async fn queue(url: &str, cfg: QueueConfig) -> (Arc<JetStreamQueue>, async_nats::Client) {
    let client = connect(url, "test").await.unwrap();
    (JetStreamQueue::new(client.clone(), cfg).await.unwrap(), client)
}

async fn cleanup(client: &async_nats::Client, stream: &str) {
    let js = async_nats::jetstream::new(client.clone());
    let _ = js.delete_stream(stream).await;
    let _ = js.delete_stream(format!("{stream}_DLQ")).await;
}

fn worker(
    q: &Arc<JetStreamQueue>,
    durable: &str,
    handler: Arc<dyn MessageHandler>,
    concurrency: usize,
) -> JetStreamWorker {
    JetStreamWorker {
        queue: q.clone(),
        durable: durable.into(),
        filter: format!("{}.>", q.config().subject_prefix),
        handler,
        concurrency,
        shutdown_grace: Duration::from_secs(5),
    }
}

async fn until(deadline: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + deadline;
    while Instant::now() < end {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    f()
}

/// Records every delivery; fails according to a per-payload plan.
#[derive(Default)]
struct Recorder {
    seen: Mutex<HashMap<String, Vec<i64>>>,
    fail_until: HashMap<String, (i64, bool)>, // payload → (fail while attempt < n, permanent?)
    hang: bool,
    started: AtomicUsize,
    sleep: Duration,
}

#[async_trait::async_trait]
impl MessageHandler for Recorder {
    async fn handle(&self, d: &Delivery<'_>) -> Result<(), HandlerError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        let key = String::from_utf8_lossy(d.payload).to_string();
        self.seen.lock().unwrap().entry(key.clone()).or_default().push(d.attempt);
        if self.hang {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
        if !self.sleep.is_zero() {
            tokio::time::sleep(self.sleep).await;
        }
        match self.fail_until.get(&key) {
            Some((n, true)) if d.attempt < *n => Err(HandlerError::Permanent(format!("bad payload {key}"))),
            Some((n, false)) if d.attempt < *n => Err(HandlerError::Retry(format!("transient failure {}", d.attempt))),
            _ => Ok(()),
        }
    }
}

#[tokio::test]
async fn event_bus_fans_out_across_instances() {
    let Some(url) = url() else { return };
    let subject = unique("events.");
    let a = NatsEventBus::start(connect(&url, "a").await.unwrap(), &subject).await.unwrap();
    let b = NatsEventBus::start(connect(&url, "b").await.unwrap(), &subject).await.unwrap();
    let (mut ra, mut rb) = (a.subscribe(), b.subscribe());
    tokio::time::sleep(Duration::from_millis(100)).await; // subscriptions registered
    a.publish(RealtimeEvent::Heartbeat { at_unix_ms: 42 }).await;
    let eb = tokio::time::timeout(Duration::from_secs(3), rb.recv()).await.unwrap().unwrap();
    let ea = tokio::time::timeout(Duration::from_secs(3), ra.recv()).await.unwrap().unwrap();
    assert_eq!(eb, RealtimeEvent::Heartbeat { at_unix_ms: 42 }, "other instance receives it");
    assert_eq!(ea, RealtimeEvent::Heartbeat { at_unix_ms: 42 }, "publisher's own subscribers receive it once");
    assert!(tokio::time::timeout(Duration::from_millis(200), ra.recv()).await.is_err(), "no duplicate locally");
}

#[tokio::test]
async fn idempotency_key_deduplicates_publishes() {
    let Some(url) = url() else { return };
    let stream = unique("TDEDUP");
    let (q, client) = queue(&url, fast_config(&stream)).await;
    let first = q.publish("email", Bytes::from_static(b"x"), Some("order-1")).await.unwrap();
    let again = q.publish("email", Bytes::from_static(b"x"), Some("order-1")).await.unwrap();
    let other = q.publish("email", Bytes::from_static(b"x"), Some("order-2")).await.unwrap();
    assert!(!first.duplicate && again.duplicate && !other.duplicate);
    assert_eq!(again.sequence, first.sequence);
    assert_eq!(q.pending().await.unwrap(), 2);
    assert!(q.publish("bad kind!", Bytes::new(), None).await.is_err(), "subject tokens are validated");
    cleanup(&client, &stream).await;
}

#[tokio::test]
async fn every_message_is_processed_and_acknowledged_once() {
    let Some(url) = url() else { return };
    let stream = unique("TONCE");
    let (q, client) = queue(&url, fast_config(&stream)).await;
    for i in 0..300 {
        q.publish("work", Bytes::from(format!("m{i}")), Some(&format!("k{i}"))).await.unwrap();
    }
    let h = Arc::new(Recorder::default());
    let stop = CancellationToken::new();
    let w = tokio::spawn(worker(&q, "workers", h.clone(), 16).run(stop.clone()));
    assert!(until(Duration::from_secs(20), || h.seen.lock().unwrap().len() == 300).await);
    tokio::time::sleep(Duration::from_millis(300)).await;
    stop.cancel();
    w.await.unwrap().unwrap();
    assert!(h.seen.lock().unwrap().values().all(|v| v == &vec![1]), "each exactly once, first attempt");
    assert_eq!(q.pending().await.unwrap(), 0, "work queue drained (acks removed messages)");
    cleanup(&client, &stream).await;
}

#[tokio::test]
async fn transient_failures_retry_with_backoff_then_succeed() {
    let Some(url) = url() else { return };
    let stream = unique("TRETRY");
    let (q, client) = queue(&url, fast_config(&stream)).await;
    q.publish("work", Bytes::from_static(b"flaky"), None).await.unwrap();
    let h = Arc::new(Recorder { fail_until: HashMap::from([("flaky".into(), (3, false))]), ..Default::default() });
    let stop = CancellationToken::new();
    let w = tokio::spawn(worker(&q, "workers", h.clone(), 2).run(stop.clone()));
    assert!(until(Duration::from_secs(10), || h.seen.lock().unwrap().get("flaky").is_some_and(|v| v.len() == 3)).await);
    tokio::time::sleep(Duration::from_millis(200)).await;
    stop.cancel();
    w.await.unwrap().unwrap();
    assert_eq!(h.seen.lock().unwrap()["flaky"], vec![1, 2, 3]);
    assert_eq!(q.pending().await.unwrap(), 0);
    assert!(q.dead_letters(10).await.unwrap().is_empty());
    cleanup(&client, &stream).await;
}

#[tokio::test]
async fn exhausted_and_permanent_failures_are_dead_lettered_and_replayable() {
    let Some(url) = url() else { return };
    let stream = unique("TDLQ");
    let (q, client) = queue(&url, fast_config(&stream)).await;
    q.publish("work", Bytes::from_static(b"always-fails"), None).await.unwrap();
    q.publish("work", Bytes::from_static(b"poison"), None).await.unwrap();
    q.publish("work", Bytes::from_static(b"fine"), None).await.unwrap();
    let h = Arc::new(Recorder {
        fail_until: HashMap::from([("always-fails".into(), (99, false)), ("poison".into(), (99, true))]),
        ..Default::default()
    });
    let stop = CancellationToken::new();
    let w = tokio::spawn(worker(&q, "workers", h.clone(), 4).run(stop.clone()));
    let q2 = q.clone();
    let deadline = Instant::now() + Duration::from_secs(10);
    while q2.dead_letters(10).await.unwrap().len() < 2 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    stop.cancel();
    w.await.unwrap().unwrap();
    {
        let seen = h.seen.lock().unwrap();
        assert_eq!(seen["always-fails"], vec![1, 2, 3], "retried up to max_deliver");
        assert_eq!(seen["poison"], vec![1], "permanent failure is not retried");
    }
    let dead = q.dead_letters(10).await.unwrap();
    assert_eq!(dead.len(), 2);
    let by: HashMap<_, _> = dead.iter().map(|d| (String::from_utf8_lossy(&d.payload).to_string(), d)).collect();
    assert_eq!(by["always-fails"].attempts, 3);
    assert!(by["always-fails"].reason.contains("transient failure"));
    assert_eq!(by["poison"].attempts, 1);
    assert!(by["poison"].reason.contains("permanent"));
    assert_eq!(by["poison"].original_subject, q.subject("work"));
    assert_eq!(q.pending().await.unwrap(), 0, "nothing stranded in the work queue");

    // Replay after a fix: messages get fresh attempts and succeed.
    assert_eq!(q.replay_dead_letters(10).await.unwrap(), 2);
    assert!(q.dead_letters(10).await.unwrap().is_empty());
    let fixed = Arc::new(Recorder::default());
    let stop = CancellationToken::new();
    let w = tokio::spawn(worker(&q, "workers", fixed.clone(), 4).run(stop.clone()));
    assert!(until(Duration::from_secs(10), || fixed.seen.lock().unwrap().len() == 2).await);
    tokio::time::sleep(Duration::from_millis(200)).await;
    stop.cancel();
    w.await.unwrap().unwrap();
    assert_eq!(q.pending().await.unwrap(), 0);
    cleanup(&client, &stream).await;
}

#[tokio::test]
async fn a_crash_on_the_last_attempt_is_swept_to_the_dead_letter_stream() {
    let Some(url) = url() else { return };
    let stream = unique("TCRASH");
    let cfg = QueueConfig { max_deliver: 2, ack_wait: Duration::from_millis(400), ..fast_config(&stream) };
    let (q, client) = queue(&url, cfg).await;
    q.publish("work", Bytes::from_static(b"crashy"), None).await.unwrap();
    // Two workers each take the message and die mid-handler (task aborted, no ack).
    for attempt in 1..=2 {
        let h = Arc::new(Recorder { hang: true, ..Default::default() });
        let w = tokio::spawn(worker(&q, "workers", h.clone(), 1).run(CancellationToken::new()));
        assert!(
            until(Duration::from_secs(5), || h.started.load(Ordering::SeqCst) == 1).await,
            "attempt {attempt} delivered"
        );
        w.abort();
        let _ = w.await;
    }
    // A healthy worker is running when the server gives up on the message.
    let h = Arc::new(Recorder::default());
    let stop = CancellationToken::new();
    let w = tokio::spawn(worker(&q, "workers", h.clone(), 1).run(stop.clone()));
    let deadline = Instant::now() + Duration::from_secs(10);
    while q.dead_letters(10).await.unwrap().is_empty() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    stop.cancel();
    w.await.unwrap().unwrap();
    let dead = q.dead_letters(10).await.unwrap();
    assert_eq!(dead.len(), 1, "stranded message preserved");
    assert!(dead[0].reason.contains("max deliveries"), "{}", dead[0].reason);
    assert_eq!(h.started.load(Ordering::SeqCst), 0, "not delivered a third time");
    assert_eq!(q.pending().await.unwrap(), 0, "removed from the work queue");
    cleanup(&client, &stream).await;
}

#[tokio::test]
async fn graceful_shutdown_finishes_in_flight_work() {
    let Some(url) = url() else { return };
    let stream = unique("TDRAIN");
    let (q, client) = queue(&url, fast_config(&stream)).await;
    for i in 0..4 {
        q.publish("work", Bytes::from(format!("d{i}")), None).await.unwrap();
    }
    let h = Arc::new(Recorder { sleep: Duration::from_millis(400), ..Default::default() });
    let stop = CancellationToken::new();
    let w = tokio::spawn(worker(&q, "workers", h.clone(), 4).run(stop.clone()));
    assert!(until(Duration::from_secs(5), || h.started.load(Ordering::SeqCst) == 4).await);
    stop.cancel(); // while all four are mid-handler
    w.await.unwrap().unwrap();
    assert_eq!(q.pending().await.unwrap(), 0, "in-flight work finished and was acknowledged");
    assert!(h.seen.lock().unwrap().values().all(|v| v == &vec![1]));
    cleanup(&client, &stream).await;
}

static TRACING: OnceLock<()> = OnceLock::new();

#[tokio::test]
async fn trace_context_travels_with_the_message() {
    let Some(url) = url() else { return };
    TRACING.get_or_init(|| {
        let cfg = app_config::TelemetryConfig { trace_propagation: true, ..Default::default() };
        std::mem::forget(
            app_telemetry::init_tracing("warn,app_messaging=info,nats=info", app_config::LogFormat::Pretty, &cfg)
                .unwrap(),
        );
    });
    struct TraceProbe(Mutex<Option<String>>);
    #[async_trait::async_trait]
    impl MessageHandler for TraceProbe {
        async fn handle(&self, _d: &Delivery<'_>) -> Result<(), HandlerError> {
            *self.0.lock().unwrap() = app_telemetry::propagation::trace_id(&tracing::Span::current());
            Ok(())
        }
    }
    let stream = unique("TTRACE");
    let (q, client) = queue(&url, fast_config(&stream)).await;
    let producer = tracing::info_span!("producer");
    let mut parent = HashMap::new();
    parent.insert("traceparent".to_string(), "00-5c1f0000000000000000000000000abc-1111111111111111-01".to_string());
    app_telemetry::propagation::set_parent_from_map(&producer, &parent);
    q.publish("work", Bytes::from_static(b"t"), None).instrument(producer).await.unwrap();
    let probe = Arc::new(TraceProbe(Mutex::new(None)));
    let stop = CancellationToken::new();
    let w = tokio::spawn(worker(&q, "workers", probe.clone(), 1).run(stop.clone()));
    assert!(until(Duration::from_secs(5), || probe.0.lock().unwrap().is_some()).await);
    stop.cancel();
    w.await.unwrap().unwrap();
    assert_eq!(probe.0.lock().unwrap().as_deref(), Some("5c1f0000000000000000000000000abc"));
    cleanup(&client, &stream).await;
}
