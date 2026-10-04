//! Liveness/readiness model.
//!
//! - **Liveness** (`/healthz`): the process is running and the event loop responds. Never
//!   depends on external systems (a database outage must not restart every pod).
//! - **Readiness** (`/readyz`): this instance should receive traffic. Critical dependencies
//!   must be reachable and the instance must not be draining.

use std::time::Duration;

#[async_trait::async_trait]
pub trait HealthCheck: Send + Sync {
    fn name(&self) -> &'static str;
    /// Critical checks make `/readyz` fail; non-critical ones are reported as degraded.
    fn critical(&self) -> bool {
        true
    }
    async fn check(&self) -> Result<(), String>;
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckResult {
    pub name: &'static str,
    pub ok: bool,
    pub critical: bool,
    pub latency_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Run all checks concurrently, each bounded by `timeout`.
pub async fn run_checks(checks: &[std::sync::Arc<dyn HealthCheck>], timeout: Duration) -> Vec<CheckResult> {
    futures::future::join_all(checks.iter().map(|c| async move {
        let start = std::time::Instant::now();
        let res = tokio::time::timeout(timeout, c.check()).await;
        let error = match res {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e),
            Err(_) => Some(format!("timed out after {}ms", timeout.as_millis())),
        };
        CheckResult {
            name: c.name(),
            ok: error.is_none(),
            critical: c.critical(),
            latency_ms: start.elapsed().as_secs_f64() * 1000.0,
            error,
        }
    }))
    .await
}
