//! HTTP surface of the application: router, middleware stack and handlers.
//!
//! Layout:
//! - [`state`]: shared application state (cheap to clone).
//! - [`middleware`]: request ids, metrics, client ip, rate limiting, load shedding, headers.
//! - [`routes`]: handlers grouped by concern.
//! - [`server`]: listener + graceful shutdown (drain, then stop).

pub mod audit;
pub mod auth;
pub mod bootstrap;
pub mod errors;
pub mod health;
pub mod middleware;
pub mod router;
pub mod routes;
pub mod server;
pub mod services;
pub mod state;

pub use router::build_router;
pub use state::{AppState, BuildInfo};
