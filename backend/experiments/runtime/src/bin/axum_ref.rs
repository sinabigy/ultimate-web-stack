//! Reference: the framework the blueprint actually uses (axum on Tokio multi-thread).
use axum::{Router, routing::get};

fn main() -> std::io::Result<()> {
    let rt =
        tokio::runtime::Builder::new_multi_thread().worker_threads(runtime_experiment::threads()).enable_all().build()?;
    rt.block_on(async {
        let app = Router::new().route("/", get(|| async { "Hello, World!" }));
        let listener = tokio::net::TcpListener::bind(runtime_experiment::addr()).await?;
        axum::serve(listener, app).await
    })
}
