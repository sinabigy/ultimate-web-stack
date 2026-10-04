use std::{collections::HashMap, sync::Arc, time::Duration};

use app_db::jobs;
use sqlx::postgres::PgListener;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

use crate::{JobContext, JobError, JobHandler, JobServices};

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub queue: String,
    pub concurrency: usize,
    pub lease: Duration,
    pub poll_interval: Duration,
    /// How long in-flight jobs get to finish (or checkpoint) after shutdown starts.
    pub shutdown_grace: Duration,
}

impl WorkerConfig {
    pub fn new(queue: &str, concurrency: usize) -> Self {
        Self {
            queue: queue.into(),
            concurrency: concurrency.max(1),
            lease: Duration::from_secs(60),
            poll_interval: Duration::from_secs(1),
            shutdown_grace: Duration::from_secs(25),
        }
    }
}

pub struct PgWorker {
    cfg: WorkerConfig,
    id: String,
    services: JobServices,
    handlers: HashMap<&'static str, Arc<dyn JobHandler>>,
}

impl PgWorker {
    pub fn new(cfg: WorkerConfig, services: JobServices, handlers: Vec<Arc<dyn JobHandler>>) -> Self {
        let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "local".into());
        Self {
            id: format!("{host}:{}:{}", std::process::id(), &uuid::Uuid::now_v7().to_string()[..8]),
            cfg,
            services,
            handlers: handlers.into_iter().map(|h| (h.kind(), h)).collect(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Run until `shutdown` is cancelled, then drain in-flight jobs (bounded by the grace period).
    pub async fn run(self, shutdown: CancellationToken) {
        let me = Arc::new(self);
        let slots = Arc::new(Semaphore::new(me.cfg.concurrency));
        let mut listener = match PgListener::connect_with(&me.services.db).await {
            Ok(mut l) => match l.listen("app_jobs").await {
                Ok(()) => Some(l),
                Err(e) => {
                    tracing::warn!(error = %e, "LISTEN failed; polling only");
                    None
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "listener unavailable; polling only");
                None
            }
        };
        let reaper = {
            let (db, stop) = (me.services.db.clone(), shutdown.clone());
            tokio::spawn(async move {
                let mut t = tokio::time::interval(Duration::from_secs(15));
                loop {
                    tokio::select! {
                        () = stop.cancelled() => return,
                        _ = t.tick() => match jobs::reap_expired(&db).await {
                            Ok(n) if n > 0 => tracing::warn!(reaped = n, "requeued jobs with expired leases"),
                            Ok(_) => {}
                            Err(e) => tracing::warn!(error = %e, "lease reaper failed"),
                        },
                    }
                }
            })
        };
        tracing::info!(worker = %me.id, queue = %me.cfg.queue, concurrency = me.cfg.concurrency, "worker started");
        let mut tasks = tokio::task::JoinSet::new();
        while !shutdown.is_cancelled() {
            let free = slots.available_permits();
            let claimed = if free > 0 {
                match jobs::claim(&me.services.db, &me.cfg.queue, &me.id, free as i64, me.cfg.lease.as_secs_f64()).await
                {
                    Ok(j) => j,
                    Err(e) => {
                        tracing::warn!(error = %e, "claim failed (database unavailable?)");
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };
            let got = !claimed.is_empty();
            for job in claimed {
                let Ok(permit) = slots.clone().acquire_owned().await else { break };
                let (me, stop) = (me.clone(), shutdown.clone());
                tasks.spawn(async move {
                    me.process(job, stop).await;
                    drop(permit);
                });
            }
            while tasks.try_join_next().is_some() {}
            if got && slots.available_permits() > 0 {
                continue; // more work may be ready right now
            }
            // Wait for a notification, a free slot, the poll interval, or shutdown.
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(me.cfg.poll_interval) => {}
                n = async {
                    match listener.as_mut() {
                        Some(l) => l.recv().await.map(|_| ()),
                        None => std::future::pending().await,
                    }
                } => {
                    if n.is_err() {
                        listener = None;
                    }
                }
                Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            }
        }
        tracing::info!(worker = %me.id, inflight = tasks.len(), "worker draining");
        let drain = async { while tasks.join_next().await.is_some() {} };
        if tokio::time::timeout(me.cfg.shutdown_grace, drain).await.is_err() {
            tracing::warn!("shutdown grace elapsed; remaining jobs will be reclaimed after their lease expires");
        }
        reaper.abort();
    }

    async fn process(&self, job: jobs::JobRow, shutdown: CancellationToken) {
        let span = tracing::info_span!("job", id = %job.id, kind = %job.kind, attempt = job.attempts,
            request_id = job.trace_context.as_ref().and_then(|t| t["request_id"].as_str()).unwrap_or(""),
            trace_id = tracing::field::Empty);
        // Continue the producer's trace (the request that enqueued this job).
        if let Some(obj) = job.trace_context.as_ref().and_then(|t| t.as_object()) {
            let map: HashMap<String, String> =
                obj.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect();
            app_telemetry::propagation::set_parent_from_map(&span, &map);
        }
        if let Some(id) = app_telemetry::propagation::trace_id(&span) {
            span.record("trace_id", id);
        }
        // `instrument`, not `span.enter()`: an entered guard held across `.await` would leak this
        // span into whatever else runs on the thread while the job is suspended.
        self.process_in_span(job, shutdown).instrument(span).await;
    }

    async fn process_in_span(&self, job: jobs::JobRow, shutdown: CancellationToken) {
        let started = std::time::Instant::now();
        let kind = job.kind.clone();
        metrics::gauge!("app_jobs_inflight", "queue" => self.cfg.queue.clone()).increment(1.0);
        // Heartbeat: extend the lease while the handler runs.
        // Aborted on drop: if this future is cancelled the lease must lapse so another worker
        // can reclaim the job.
        let hb = {
            let (db, id, worker, lease) = (self.services.db.clone(), job.id, self.id.clone(), self.cfg.lease);
            app_messaging::AbortOnDrop(tokio::spawn(async move {
                let mut t = tokio::time::interval(lease / 3);
                t.tick().await;
                loop {
                    t.tick().await;
                    if !matches!(jobs::heartbeat(&db, id, &worker, lease.as_secs_f64()).await, Ok(true)) {
                        tracing::warn!(job = %id, "lease lost");
                        return;
                    }
                }
            }))
        };
        let result = match self.handlers.get(kind.as_str()) {
            Some(h) => h.handle(&JobContext { services: &self.services, job: &job, shutdown }).await,
            None => Err(JobError::Permanent(format!("no handler for job kind {kind}"))),
        };
        drop(hb);
        let label = match &result {
            Ok(()) => {
                if let Err(e) = jobs::complete(&self.services.db, job.id, &self.id).await {
                    tracing::error!(error = %e, "could not mark job complete");
                }
                "succeeded"
            }
            Err(e) => {
                let permanent = matches!(e, JobError::Permanent(_));
                match jobs::fail(&self.services.db, job.id, &self.id, &e.to_string(), permanent).await {
                    Ok(status) if status == "dead" => {
                        tracing::error!(error = %e, "job dead-lettered");
                        "dead"
                    }
                    Ok(_) => {
                        tracing::warn!(error = %e, "job failed; will retry");
                        "retry"
                    }
                    Err(err) => {
                        tracing::error!(error = %err, "could not record job failure");
                        "error"
                    }
                }
            }
        };
        metrics::gauge!("app_jobs_inflight", "queue" => self.cfg.queue.clone()).decrement(1.0);
        metrics::counter!("app_jobs_processed_total", "queue" => self.cfg.queue.clone(), "kind" => kind.clone(), "result" => label).increment(1);
        metrics::histogram!("app_job_duration_seconds", "kind" => kind).record(started.elapsed().as_secs_f64());
    }
}
