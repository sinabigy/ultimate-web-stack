//! Cookie construction. The session cookie is `HttpOnly` (no script access), `SameSite=Lax`
//! (not sent on cross-site subrequests or POSTs), `Path=/`, never `Domain` (host-only), and
//! `Secure` + `__Host-` prefix whenever `auth.cookie_secure` is on.

use app_config::{AuthConfig, SameSite};
use axum::http::{HeaderMap, HeaderValue, header};
use time::OffsetDateTime;

pub fn get(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

fn attrs(cfg: &AuthConfig, path: &str, max_age: i64) -> String {
    let same_site = match cfg.cookie_same_site {
        SameSite::Lax => "Lax",
        SameSite::Strict => "Strict",
    };
    let secure = if cfg.cookie_secure { "; Secure" } else { "" };
    format!("; Path={path}; HttpOnly; SameSite={same_site}; Max-Age={max_age}{secure}")
}

pub fn session_cookie(
    cfg: &AuthConfig,
    token: &str,
    expires_at: OffsetDateTime,
) -> Result<HeaderValue, axum::http::header::InvalidHeaderValue> {
    let max_age = (expires_at - OffsetDateTime::now_utc()).whole_seconds().max(0);
    HeaderValue::from_str(&format!("{}={token}{}", cfg.effective_cookie_name(), attrs(cfg, "/", max_age)))
}

pub fn clear_session_cookie(cfg: &AuthConfig) -> Result<HeaderValue, axum::http::header::InvalidHeaderValue> {
    HeaderValue::from_str(&format!("{}={}", cfg.effective_cookie_name(), attrs(cfg, "/", 0)))
}

/// Binds an in-flight OIDC login to this browser (login-CSRF protection). Path-scoped to /auth.
pub fn flow_cookie_name(cfg: &AuthConfig) -> String {
    format!(
        "{}_flow",
        if cfg.cookie_secure { format!("__Secure-{}", cfg.cookie_name) } else { cfg.cookie_name.clone() }
    )
}

pub fn flow_cookie(cfg: &AuthConfig, state: &str) -> Result<HeaderValue, axum::http::header::InvalidHeaderValue> {
    HeaderValue::from_str(&format!("{}={state}{}", flow_cookie_name(cfg), attrs(cfg, "/auth", 600)))
}

pub fn clear_flow_cookie(cfg: &AuthConfig) -> Result<HeaderValue, axum::http::header::InvalidHeaderValue> {
    HeaderValue::from_str(&format!("{}={}", flow_cookie_name(cfg), attrs(cfg, "/auth", 0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_cookies_are_host_prefixed_httponly_lax() {
        let cfg = AuthConfig { cookie_secure: true, ..Default::default() };
        let c = session_cookie(&cfg, "tok", OffsetDateTime::now_utc() + time::Duration::hours(1))
            .unwrap_or(HeaderValue::from_static(""));
        let s = c.to_str().unwrap_or_default();
        assert!(s.starts_with("__Host-app_session=tok; Path=/; HttpOnly; SameSite=Lax; Max-Age="), "{s}");
        assert!(s.ends_with("; Secure"));
        assert!(!s.contains("Domain"));
    }

    #[test]
    fn cookie_parsing_finds_exact_name() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("app_session_flow=x; app_session=abc; other=1"));
        assert_eq!(get(&h, "app_session").as_deref(), Some("abc"));
        assert_eq!(get(&h, "missing"), None);
    }
}
