//! Operational endpoints: liveness, readiness, version, metrics.

use std::time::Duration;

use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::{health::run_checks, state::AppState};

pub async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

pub async fn readyz(State(state): State<AppState>) -> Response {
    if state.lifecycle.is_draining() {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "status": "draining" }))).into_response();
    }
    let checks = run_checks(&state.health, Duration::from_secs(2)).await;
    let ready = checks.iter().all(|c| c.ok || !c.critical);
    let degraded = checks.iter().any(|c| !c.ok);
    let status = if !ready {
        "unavailable"
    } else if degraded {
        "degraded"
    } else {
        "ok"
    };
    let code = if ready { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    (code, Json(json!({ "status": status, "checks": checks }))).into_response()
}

pub async fn version(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.build.clone())
}

pub async fn metrics(State(state): State<AppState>) -> Response {
    match &state.metrics {
        Some(h) => ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], h.render()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
