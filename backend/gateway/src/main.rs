//! Edge gateway (hyperscale profile) built on Pingora: round-robin load balancing across
//! app-server instances with TCP health checks and pooled upstream connections.
//!
//! Configuration (environment):
//! - `GATEWAY_LISTEN` (default `0.0.0.0:18100`)
//! - `GATEWAY_UPSTREAMS` comma-separated `host:port` list (default `127.0.0.1:8080`)
//! - `GATEWAY_THREADS` worker threads (default: available parallelism)
//!
//! Measured overhead vs direct access: docs/benchmarks/pingora.md. Adopt it for what it adds
//! (one entry point, health-checked balancing, connection reuse, edge policies), not for speed.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use pingora::{
    Result,
    http::RequestHeader,
    lb::{LoadBalancer, health_check::TcpHealthCheck, selection::RoundRobin},
    prelude::{HttpPeer, Opt},
    proxy::{ProxyHttp, Session, http_proxy_service},
    server::{Server, configuration::ServerConf},
    services::background::background_service,
};

struct Gateway {
    upstreams: Arc<LoadBalancer<RoundRobin>>,
}

#[async_trait]
impl ProxyHttp for Gateway {
    type CTX = ();
    fn new_ctx(&self) {}

    async fn upstream_peer(&self, _session: &mut Session, _ctx: &mut ()) -> Result<Box<HttpPeer>> {
        let backend = self
            .upstreams
            .select(b"", 256)
            .ok_or_else(|| pingora::Error::explain(pingora::ErrorType::ConnectNoRoute, "no healthy upstream"))?;
        // Plain HTTP to the app tier (TLS terminates here at the edge).
        let mut peer = HttpPeer::new(backend, false, String::new());
        peer.options.connection_timeout = Some(Duration::from_secs(2));
        peer.options.read_timeout = Some(Duration::from_secs(30));
        peer.options.idle_timeout = Some(Duration::from_secs(60));
        Ok(Box::new(peer))
    }

    async fn upstream_request_filter(
        &self,
        session: &mut Session,
        upstream_request: &mut RequestHeader,
        _ctx: &mut (),
    ) -> Result<()> {
        // The app trusts X-Forwarded-For only when configured to (http.trust_forwarded_for).
        if let Some(addr) = session.client_addr().and_then(|a| a.as_inet()).map(|a| a.ip().to_string()) {
            let xff = match upstream_request.headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
                Some(prev) => format!("{prev}, {addr}"),
                None => addr,
            };
            upstream_request.insert_header("x-forwarded-for", xff)?;
        }
        Ok(())
    }
}

fn main() {
    let listen = std::env::var("GATEWAY_LISTEN").unwrap_or_else(|_| "0.0.0.0:18100".into());
    let upstreams_env = std::env::var("GATEWAY_UPSTREAMS").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let threads = std::env::var("GATEWAY_THREADS")
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));

    let conf = ServerConf { threads, ..ServerConf::default() };
    let mut server = Server::new_with_opt_and_conf(Opt::default(), conf);
    server.bootstrap();

    let mut upstreams = LoadBalancer::try_from_iter(upstreams_env.split(',').map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or_else(|e| panic!("invalid GATEWAY_UPSTREAMS {upstreams_env:?}: {e}"));
    upstreams.set_health_check(TcpHealthCheck::new());
    upstreams.health_check_frequency = Some(Duration::from_secs(1));
    let health = background_service("upstream health check", upstreams);

    let mut proxy = http_proxy_service(&server.configuration, Gateway { upstreams: health.task() });
    proxy.add_tcp(&listen);
    server.add_service(proxy);
    server.add_service(health);
    eprintln!("gateway listening on {listen} → {upstreams_env} ({threads} threads)");
    server.run_forever();
}
