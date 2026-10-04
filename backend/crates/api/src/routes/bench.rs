//! Benchmark endpoints (enabled only with `http.bench_endpoints`; refused in production).
//! Shapes follow TechEmpower's plaintext and JSON tests so results are comparable in kind
//! (not in absolute numbers: different hardware, middleware stack and methodology).

use axum::{Json, http::header, response::IntoResponse};
use serde::Serialize;

pub async fn plaintext() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/plain")], "Hello, World!")
}

#[derive(Serialize)]
pub struct Message {
    message: &'static str,
}

pub async fn json() -> impl IntoResponse {
    Json(Message { message: "Hello, World!" })
}
