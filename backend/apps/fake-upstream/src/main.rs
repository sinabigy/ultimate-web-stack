//! `fake-upstream [port]`: run the simulated provider (default 127.0.0.1:59090).
//! Configure behaviour with `POST /__control` (JSON `Behaviour`), read counters at `GET /__stats`.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().compact().init();
    let port: u16 = std::env::args().nth(1).and_then(|p| p.parse().ok()).unwrap_or(59090);
    let host: std::net::IpAddr =
        std::env::var("FAKE_UPSTREAM_HOST").ok().and_then(|h| h.parse().ok()).unwrap_or([127, 0, 0, 1].into());
    let r = fake_upstream::start((host, port).into(), fake_upstream::Behaviour::default()).await?;
    tracing::info!(addr = %r.addr, "fake upstream listening");
    tokio::signal::ctrl_c().await?;
    Ok(())
}
