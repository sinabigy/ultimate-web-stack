//! Serving with graceful shutdown.
//!
//! On SIGTERM/SIGINT:
//! 1. `/readyz` starts returning 503 (`draining`) so load balancers stop sending traffic;
//! 2. after `http.shutdown_drain_ms` the listener stops accepting and streams are closed;
//! 3. in-flight requests get up to `http.shutdown_timeout_ms` to finish, then the process exits.

use std::{io, net::SocketAddr};

use axum::{Router, serve::ListenerExt};
use tokio::net::TcpListener;

use crate::state::AppState;

pub async fn serve(listener: TcpListener, router: Router, state: AppState) -> io::Result<()> {
    let listener = listener.tap_io(|tcp| {
        let _ = tcp.set_nodelay(true);
    });
    let stop = state.lifecycle.shutdown.clone();
    let server = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async move { stop.cancelled().await });
    let mut handle = tokio::spawn(async move { server.await });

    tokio::select! {
        res = &mut handle => return res.map_err(io::Error::other)?,
        () = shutdown_signal() => {}
        () = state.lifecycle.shutdown.cancelled() => {}
    }
    tracing::info!(drain_ms = state.config.http.shutdown_drain_ms, "shutdown requested: draining");
    state.lifecycle.start_draining();
    tokio::time::sleep(state.config.http.shutdown_drain()).await;
    state.lifecycle.shutdown.cancel();
    match tokio::time::timeout(state.config.http.shutdown_timeout(), &mut handle).await {
        Ok(res) => {
            tracing::info!("server stopped cleanly");
            res.map_err(io::Error::other)?
        }
        Err(_) => {
            tracing::warn!("shutdown timeout elapsed; aborting remaining connections");
            handle.abort();
            Ok(())
        }
    }
}

/// Resolves on SIGINT (Ctrl-C) or SIGTERM.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = term => {}
    }
}
