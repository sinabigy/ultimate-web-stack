#![allow(dead_code, clippy::unwrap_used)]
//! Shared helpers for API integration tests.

use std::sync::Arc;

use app_api::{AppState, BuildInfo, health::HealthCheck};
use app_config::AppConfig;
use axum::{
    Router,
    body::Body,
    http::{Request, Response},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

pub fn build_info() -> BuildInfo {
    BuildInfo { name: "app-server", version: "0.0.0-test", git_sha: "test", profile: "debug" }
}

pub fn test_config() -> AppConfig {
    let mut c = AppConfig { environment: "test".into(), ..Default::default() };
    c.http.bench_endpoints = true;
    c.rate_limit.enabled = false;
    c
}

pub struct StaticCheck(pub &'static str, pub bool, pub Result<(), String>);

#[async_trait::async_trait]
impl HealthCheck for StaticCheck {
    fn name(&self) -> &'static str {
        self.0
    }
    fn critical(&self) -> bool {
        self.1
    }
    async fn check(&self) -> Result<(), String> {
        self.2.clone()
    }
}

pub fn state_with(config: AppConfig, checks: Vec<Arc<dyn HealthCheck>>) -> AppState {
    let mut b = AppState::builder(config, build_info());
    for c in checks {
        b = b.health_check(c);
    }
    b.build()
}

pub async fn send(app: &Router, req: Request<Body>) -> Response<Body> {
    app.clone().oneshot(req).await.unwrap()
}

pub async fn body_json(res: Response<Body>) -> serde_json::Value {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

pub fn get_req(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}
