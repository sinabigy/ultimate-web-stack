//! HTTP client for identity-provider calls (discovery, token, JWKS).
//! Redirects are disabled (an OIDC endpoint must not bounce us elsewhere: SSRF guard) and
//! every request has a short timeout.

use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("invalid request: {0}")]
    Build(String),
}

pub fn build(timeout: Duration) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(3))
        .user_agent(concat!("app-auth/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// Adapter so `openidconnect` uses our reqwest client.
pub async fn execute(
    client: &reqwest::Client,
    req: openidconnect::HttpRequest,
) -> Result<openidconnect::HttpResponse, HttpError> {
    let (parts, body) = req.into_parts();
    let mut builder = client.request(parts.method, parts.uri.to_string()).body(body);
    for (k, v) in &parts.headers {
        builder = builder.header(k, v);
    }
    let resp = builder.send().await?;
    let mut out = http::Response::builder().status(resp.status());
    for (k, v) in resp.headers() {
        out = out.header(k, v);
    }
    let bytes = resp.bytes().await?;
    out.body(bytes.to_vec()).map_err(|e| HttpError::Build(e.to_string()))
}
