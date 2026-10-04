//! Job handlers for the example application.

use std::time::{Duration, Instant};

use app_domain::{RealtimeEvent, RunStatus};
use app_networking::{CallError, CallRequest, Priority};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::{JobContext, JobError, JobHandler};

#[derive(Deserialize)]
struct RunPayload {
    run_id: Uuid,
    organization_id: Uuid,
}

/// Execute a run: `requested` calls to the run's provider through the outbound engine.
///
/// Idempotent under redelivery and crashes:
/// - resumes from the stored counters (calls already done are not repeated);
/// - each call carries a stable idempotency key `run:<id>:<n>`, so a call that reached the
///   provider before a crash is deduplicated by a provider that honours the key;
/// - progress counters are written as absolute values (`GREATEST`), never increments.
pub struct ExecuteRun {
    pub parallelism: usize,
}

#[async_trait::async_trait]
impl JobHandler for ExecuteRun {
    fn kind(&self) -> &'static str {
        "execute_run"
    }

    async fn handle(&self, ctx: &JobContext<'_>) -> Result<(), JobError> {
        let p: RunPayload = serde_json::from_value(ctx.job.payload.clone())
            .map_err(|e| JobError::Permanent(format!("bad payload: {e}")))?;
        let svc = ctx.services;
        let Some(run) = app_db::runs::job_start(&svc.db, p.organization_id, p.run_id)
            .await
            .map_err(|e| JobError::Retryable(e.to_string()))?
        else {
            return Ok(()); // run deleted or already finished: nothing to do
        };
        let Some(provider) = svc.providers.get(&run.provider).cloned() else {
            app_db::runs::job_finish(&svc.db, run.organization_id, run.id, RunStatus::Failed).await.ok();
            return Err(JobError::Permanent(format!("unknown provider {}", run.provider)));
        };
        let start_at = (run.succeeded + run.failed).max(0) as usize;
        let total = run.requested.max(0) as usize;
        let (mut ok, mut failed) = (run.succeeded, run.failed);
        let report_every = (total / 20).max(1);
        let mut last_report = Instant::now();
        let mut since_report = 0usize;
        let run_id = run.id;
        let calls = futures::stream::iter(start_at..total)
            .map(|n| {
                let provider = provider.clone();
                async move {
                    provider
                        .call(
                            CallRequest::post("/v1/echo", json!({"run": run_id, "n": n, "tokens": 1}))
                                .idempotency_key(format!("run:{run_id}:{n}"))
                                .priority(Priority::BACKGROUND)
                                .deadline(Duration::from_secs(60)),
                        )
                        .await
                }
            })
            .buffer_unordered(self.parallelism.max(1));
        tokio::pin!(calls);
        let mut interrupted: Option<JobError> = None;
        loop {
            let next = tokio::select! {
                biased;
                () = ctx.shutdown.cancelled() => {
                    interrupted = Some(JobError::Retryable("interrupted by shutdown; progress saved".into()));
                    break;
                }
                n = calls.next() => n,
            };
            let Some(result) = next else { break };
            match result {
                Ok(_) => ok += 1,
                // The provider is down: stop and retry the job later rather than burning calls.
                Err(CallError::CircuitOpen) => {
                    interrupted = Some(JobError::Retryable("provider circuit open; progress saved".into()));
                    break;
                }
                Err(e) => {
                    tracing::debug!(error = %e, "provider call failed");
                    failed += 1;
                }
            }
            since_report += 1;
            if since_report >= report_every || last_report.elapsed() > Duration::from_millis(250) {
                since_report = 0;
                last_report = Instant::now();
                app_db::runs::job_progress(&svc.db, run.organization_id, run.id, ok, failed)
                    .await
                    .map_err(|e| JobError::Retryable(e.to_string()))?;
                svc.events
                    .publish(RealtimeEvent::RunProgress {
                        run_id: run.id,
                        organization_id: run.organization_id,
                        succeeded: ok,
                        failed,
                        requested: run.requested,
                    })
                    .await;
            }
        }
        app_db::runs::job_progress(&svc.db, run.organization_id, run.id, ok, failed)
            .await
            .map_err(|e| JobError::Retryable(e.to_string()))?;
        if let Some(e) = interrupted {
            return Err(e);
        }
        let status = if ok > 0 || run.requested == 0 { RunStatus::Completed } else { RunStatus::Failed };
        app_db::runs::job_finish(&svc.db, run.organization_id, run.id, status)
            .await
            .map_err(|e| JobError::Retryable(e.to_string()))?;
        svc.analytics.record(
            app_analytics::EventRow::new("run_finished", run.organization_id)
                .user((run.owner_id != Uuid::nil()).then_some(run.owner_id))
                .value(f64::from(ok))
                .properties(&serde_json::json!({"status": status, "failed": failed, "requested": run.requested})),
        );
        svc.events
            .publish(RealtimeEvent::RunFinished {
                run_id: run.id,
                organization_id: run.organization_id,
                status,
                succeeded: ok,
                failed,
            })
            .await;
        if run.owner_id != Uuid::nil() {
            let title =
                format!("Run “{}” {}", run.label, if status == RunStatus::Completed { "completed" } else { "failed" });
            let body = format!("{ok} succeeded, {failed} failed of {} calls", run.requested);
            if let Ok(id) = app_db::notifications::create(
                &svc.db,
                run.owner_id,
                Some(run.organization_id),
                "run",
                &title,
                &body,
                None,
            )
            .await
            {
                svc.events
                    .publish(RealtimeEvent::Notification { user_id: run.owner_id, notification_id: id, title })
                    .await;
            }
        }
        Ok(())
    }
}
