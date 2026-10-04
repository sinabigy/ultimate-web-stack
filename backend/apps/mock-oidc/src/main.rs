//! `mock-oidc`: local OIDC provider for offline development (`./dev up --auth mock`).
//! Configured from environment: MOCK_OIDC_PORT, MOCK_OIDC_CLIENT_ID, MOCK_OIDC_REDIRECT_URIS,
//! MOCK_OIDC_POST_LOGOUT_URIS, MOCK_OIDC_SERVICE_AUDIENCE.

use std::net::SocketAddr;

fn env_list(name: &str, default: &str) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_else(|_| default.into())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().compact().init();
    if std::env::var("APP__ENVIRONMENT").is_ok_and(|e| e == "production") {
        anyhow::bail!("mock-oidc refuses to run with APP__ENVIRONMENT=production");
    }
    let port: u16 = std::env::var("MOCK_OIDC_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(59081);
    let cfg = mock_oidc::MockConfig {
        clients: vec![mock_oidc::MockClient {
            id: std::env::var("MOCK_OIDC_CLIENT_ID").unwrap_or_else(|_| "app-web".into()),
            secret: None,
            redirect_uris: env_list(
                "MOCK_OIDC_REDIRECT_URIS",
                "http://localhost:5190/auth/callback,http://localhost:8080/auth/callback",
            ),
            post_logout_redirect_uris: env_list(
                "MOCK_OIDC_POST_LOGOUT_URIS",
                "http://localhost:5190/login,http://localhost:8080/login",
            ),
        }],
        service_accounts: vec![("svc-reporting".into(), "svc-secret-dev-only".into(), vec!["runs:read".into()])],
        service_audience: std::env::var("MOCK_OIDC_SERVICE_AUDIENCE").unwrap_or_else(|_| "app-api".into()),
        roles_claim: "urn:zitadel:iam:org:project:roles".into(),
    };
    let running = mock_oidc::start(SocketAddr::from(([127, 0, 0, 1], port)), cfg).await?;
    tracing::warn!(issuer = %running.issuer, "mock OIDC provider running — development only, never deploy");
    tokio::signal::ctrl_c().await?;
    Ok(())
}
