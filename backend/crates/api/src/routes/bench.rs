//! Benchmark endpoints (enabled only with `http.bench_endpoints`; refused in production).
//! Shapes follow TechEmpower's plaintext and JSON tests so results are comparable in kind
//! (not in absolute numbers: different hardware, middleware stack and methodology).

use axum::{
    Json,
    extract::State,
    http::header,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::{errors::ResultExt, state::AppState};

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

/// A typical list page: 30 rows shaped like `GET /api/v1/orgs/{slug}/audit` (about 13 KB),
/// with fresh identifiers so compression sees realistic entropy.
pub async fn list() -> impl IntoResponse {
    let now =
        time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default();
    let (actor, org) = (uuid::Uuid::now_v7(), uuid::Uuid::now_v7());
    let items: Vec<serde_json::Value> = (0..30)
        .map(|_| {
            serde_json::json!({
                "id": uuid::Uuid::now_v7(), "occurred_at": now, "actor_type": "user", "actor_id": actor,
                "actor_label": "someone@example.com", "action": "run.created", "outcome": "success",
                "target_type": "run", "target_id": uuid::Uuid::now_v7(), "organization_id": org,
                "request_id": uuid::Uuid::new_v4(), "metadata": {},
            })
        })
        .collect();
    Json(serde_json::json!({ "items": items, "next_cursor": null }))
}

#[derive(Serialize, serde::Deserialize)]
pub struct World {
    id: i32,
    random_number: i32,
}

fn random_id() -> i32 {
    let mut b = [0u8; 4];
    rand::fill(&mut b);
    (u32::from_le_bytes(b) % 10_000) as i32 + 1
}

/// TechEmpower "single query": one indexed row by primary key.
pub async fn db_read(State(state): State<AppState>) -> Result<Response, app_errors::ApiError> {
    let svc = state.svc()?;
    let (id, random_number): (i32, i32) = sqlx::query_as("SELECT id, random_number FROM bench_world WHERE id = $1")
        .bind(random_id())
        .fetch_one(&svc.db)
        .await
        .api()?;
    Ok(Json(World { id, random_number }).into_response())
}

/// TechEmpower "updates" (single row): read + write in one statement.
pub async fn db_write(State(state): State<AppState>) -> Result<Response, app_errors::ApiError> {
    let svc = state.svc()?;
    let (id, random_number): (i32, i32) =
        sqlx::query_as("UPDATE bench_world SET random_number = $2 WHERE id = $1 RETURNING id, random_number")
            .bind(random_id())
            .bind(random_id())
            .fetch_one(&svc.db)
            .await
            .api()?;
    Ok(Json(World { id, random_number }).into_response())
}

/// Cached read: a database row served from the configured cache (memory or Redis).
pub async fn cached_read(State(state): State<AppState>) -> Result<Response, app_errors::ApiError> {
    let svc = state.svc()?;
    let id = random_id() % 100 + 1; // hot set of 100 keys
    let key = state.cache.key("bench-world", 1, &[&id.to_string()]);
    let world: World = state
        .cache
        .get_or_load(&key, std::time::Duration::from_secs(60), || async {
            let (id, random_number): (i32, i32) =
                sqlx::query_as("SELECT id, random_number FROM bench_world WHERE id = $1")
                    .bind(id)
                    .fetch_one(&svc.db)
                    .await?;
            Ok::<_, sqlx::Error>(World { id, random_number })
        })
        .await
        .api()?;
    Ok(Json(world).into_response())
}
