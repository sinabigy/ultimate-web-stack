//! OpenID Connect relying party for the BFF (authorization code + PKCE S256 + nonce + state).
//!
//! The browser never sees a token: the backend performs the code exchange, verifies the
//! ID token (signature via the provider's JWKS, issuer, audience, expiry, nonce) and creates
//! its own opaque session. Provider metadata is discovered lazily and cached, so the API
//! starts even when the IdP is unreachable (login then returns 503 and `/readyz` reports it).

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use app_config::{AuthConfig, IdentityProviderKind};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use openidconnect::{
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, LoginHint, LogoutRequest, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier,
    PostLogoutRedirectUrl, ProviderMetadataWithLogout, RedirectUrl, Scope, TokenResponse,
    core::{CoreAuthPrompt, CoreClient, CoreJwsSigningAlgorithm, CoreResponseType, CoreUserInfoClaims},
};
use time::OffsetDateTime;
use tokio::sync::RwLock;

use crate::http_client;

type Client =
    CoreClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointMaybeSet, EndpointMaybeSet>;

/// How long discovered metadata (including JWKS) is trusted before refetching.
const METADATA_TTL: Duration = Duration::from_secs(3600);
/// Minimum interval between forced refreshes (unknown signing key), to resist key-id spam.
const MIN_FORCED_REFRESH: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum OidcError {
    #[error("identity provider unavailable: {0}")]
    Unavailable(String),
    #[error("identity provider misconfigured: {0}")]
    Config(String),
    #[error("authorization code exchange failed: {0}")]
    Exchange(String),
    #[error("ID token rejected: {0}")]
    InvalidToken(String),
    #[error("identity provider returned an error: {0}")]
    Provider(String),
    #[error("required claim missing: {0}")]
    MissingClaim(&'static str),
}

/// What the user asked for when starting a login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Login,
    Register,
    /// Step-up: force fresh authentication (e.g. before account deletion or admin actions).
    Reauth,
}

impl Intent {
    pub fn as_str(self) -> &'static str {
        match self {
            Intent::Login => "login",
            Intent::Register => "register",
            Intent::Reauth => "reauth",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "login" => Some(Intent::Login),
            "register" => Some(Intent::Register),
            "reauth" => Some(Intent::Reauth),
            _ => None,
        }
    }
}

/// Optional login method selection from the application's login page.
#[derive(Debug, Clone, Default)]
pub struct LoginOptions {
    pub login_hint: Option<String>,
    /// Name of a configured social/enterprise IdP (must exist in `auth.methods.social`).
    pub social: Option<String>,
}

/// Everything the caller must persist (server-side) to complete the flow.
pub struct PendingLogin {
    pub authorize_url: String,
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

/// Verified identity from a completed login. Contains no tokens except the raw ID token,
/// which the caller encrypts and keeps only for RP-initiated logout.
#[derive(Debug, Clone)]
pub struct VerifiedLogin {
    pub issuer: String,
    pub subject: String,
    pub email: String,
    pub email_verified: bool,
    pub name: Option<String>,
    pub picture: Option<String>,
    pub amr: Vec<String>,
    pub auth_time: OffsetDateTime,
    /// Roles found in the configured system-role claim (empty if absent).
    pub idp_roles: Vec<String>,
    pub id_token: String,
}

struct Discovered {
    client: Client,
    end_session: Option<openidconnect::EndSessionUrl>,
    fetched_at: Instant,
}

pub struct OidcProvider {
    cfg: AuthConfig,
    http: reqwest::Client,
    state: RwLock<Option<Arc<Discovered>>>,
    last_forced: std::sync::Mutex<Option<Instant>>,
}

impl OidcProvider {
    pub fn new(cfg: AuthConfig) -> Result<Self, OidcError> {
        let http = http_client::build(Duration::from_secs(10)).map_err(|e| OidcError::Config(e.to_string()))?;
        Ok(Self { cfg, http, state: RwLock::new(None), last_forced: std::sync::Mutex::new(None) })
    }

    pub fn issuer(&self) -> &str {
        &self.cfg.issuer_url
    }

    async fn discover(&self) -> Result<Arc<Discovered>, OidcError> {
        let issuer = IssuerUrl::new(self.cfg.issuer_url.clone()).map_err(|e| OidcError::Config(e.to_string()))?;
        let http = self.http.clone();
        let fetch = move |req| {
            let http = http.clone();
            async move { http_client::execute(&http, req).await }
        };
        let meta = ProviderMetadataWithLogout::discover_async(issuer, &fetch)
            .await
            .map_err(|e| OidcError::Unavailable(error_chain(&e)))?;
        let end_session = meta.additional_metadata().end_session_endpoint.clone();
        let secret = (!self.cfg.client_secret.is_empty())
            .then(|| ClientSecret::new(self.cfg.client_secret.expose().to_string()));
        let client = CoreClient::from_provider_metadata(meta, ClientId::new(self.cfg.client_id.clone()), secret)
            .set_redirect_uri(
                RedirectUrl::new(self.cfg.redirect_url.clone()).map_err(|e| OidcError::Config(e.to_string()))?,
            );
        tracing::info!(issuer = %self.cfg.issuer_url, "OIDC provider metadata discovered");
        Ok(Arc::new(Discovered { client, end_session, fetched_at: Instant::now() }))
    }

    async fn current(&self, force: bool) -> Result<Arc<Discovered>, OidcError> {
        if !force
            && let Some(d) = self.state.read().await.as_ref()
            && d.fetched_at.elapsed() < METADATA_TTL
        {
            return Ok(d.clone());
        }
        let mut w = self.state.write().await;
        if !force
            && let Some(d) = w.as_ref()
            && d.fetched_at.elapsed() < METADATA_TTL
        {
            return Ok(d.clone());
        }
        let d = self.discover().await?;
        *w = Some(d.clone());
        Ok(d)
    }

    /// Readiness probe: discovery reachable and well-formed.
    pub async fn check(&self) -> Result<(), String> {
        self.current(false).await.map(|_| ()).map_err(|e| e.to_string())
    }

    /// Build the authorization redirect. The caller stores `state` (hashed), `nonce` and the
    /// PKCE verifier server-side and binds `state` to the browser with a short-lived cookie.
    pub async fn begin(&self, intent: Intent, opts: &LoginOptions) -> Result<PendingLogin, OidcError> {
        let d = self.current(false).await?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut req = d.client.authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );
        for s in &self.cfg.scopes {
            if s != "openid" {
                req = req.add_scope(Scope::new(s.clone()));
            }
        }
        if self.cfg.provider == IdentityProviderKind::Zitadel && self.cfg.system_roles_from_idp {
            req = req.add_scope(Scope::new("urn:zitadel:iam:org:project:roles".into()));
        }
        req = req.set_pkce_challenge(challenge);
        if let Some(hint) = opts.login_hint.as_deref().filter(|h| !h.is_empty() && h.len() <= 254) {
            req = req.set_login_hint(LoginHint::new(hint.to_string()));
        }
        match intent {
            Intent::Login => {}
            // OpenID Connect "Initiating User Registration" (prompt=create).
            Intent::Register => req = req.add_prompt(CoreAuthPrompt::Extension("create".into())),
            Intent::Reauth => req = req.add_prompt(CoreAuthPrompt::Login).set_max_age(Duration::ZERO),
        }
        if let Some(name) = &opts.social {
            let idp_id =
                self.cfg.methods.social.get(name).ok_or(OidcError::Config(format!("unknown login method {name}")))?;
            req = match self.cfg.provider {
                // ZITADEL: skip its login page and go straight to the federated IdP.
                IdentityProviderKind::Zitadel => {
                    req.add_scope(Scope::new(format!("urn:zitadel:iam:org:idp:id:{idp_id}")))
                }
                // Common convention (Keycloak `kc_idp_hint`, Auth0 `connection` differ); configurable later.
                IdentityProviderKind::Oidc => req.add_extra_param("idp_hint", idp_id.clone()),
            };
        }
        let (url, state, nonce) = req.url();
        Ok(PendingLogin {
            authorize_url: url.to_string(),
            state: state.secret().clone(),
            nonce: nonce.secret().clone(),
            pkce_verifier: verifier.secret().clone(),
        })
    }

    /// Exchange the code and verify the ID token. Retries verification once with refreshed
    /// keys if the provider rotated its signing key.
    pub async fn complete(&self, code: &str, nonce: &str, pkce_verifier: &str) -> Result<VerifiedLogin, OidcError> {
        let d = self.current(false).await?;
        let http = self.http.clone();
        let fetch = move |req| {
            let http = http.clone();
            async move { http_client::execute(&http, req).await }
        };
        let token = d
            .client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .map_err(|e| OidcError::Config(e.to_string()))?
            .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.to_string()))
            .request_async(&fetch)
            .await
            .map_err(|e| OidcError::Exchange(error_chain(&e)))?;
        let id_token = token.id_token().ok_or(OidcError::MissingClaim("id_token"))?.clone();
        let nonce = Nonce::new(nonce.to_string());

        let verify = |client: &Client| -> Result<VerifiedLogin, OidcError> {
            let verifier = client.id_token_verifier().set_allowed_algs(vec![
                CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
                CoreJwsSigningAlgorithm::RsaSsaPssSha256,
                CoreJwsSigningAlgorithm::EcdsaP256Sha256,
                CoreJwsSigningAlgorithm::EdDsa,
            ]);
            let claims = id_token.claims(&verifier, &nonce).map_err(|e| OidcError::InvalidToken(error_chain(&e)))?;
            let auth_time = claims
                .auth_time()
                .map(|t| t.timestamp())
                .and_then(|s| OffsetDateTime::from_unix_timestamp(s).ok())
                .unwrap_or_else(OffsetDateTime::now_utc);
            let raw = id_token.to_string();
            Ok(VerifiedLogin {
                issuer: claims.issuer().to_string(),
                subject: claims.subject().to_string(),
                email: claims.email().map(|e| e.to_string()).unwrap_or_default(),
                email_verified: claims.email_verified().unwrap_or(false),
                name: claims.name().and_then(|n| n.get(None)).map(|n| n.to_string()),
                picture: claims.picture().and_then(|p| p.get(None)).map(|p| p.to_string()),
                amr: claims.auth_method_refs().map(|v| v.iter().map(|a| a.to_string()).collect()).unwrap_or_default(),
                auth_time,
                idp_roles: extract_roles(&raw, &self.cfg.system_role_claim),
                id_token: raw,
            })
        };

        let mut login = match verify(&d.client) {
            Ok(l) => l,
            Err(OidcError::InvalidToken(msg)) if self.may_force_refresh() => {
                tracing::info!(reason = %msg, "ID token verification failed; refreshing provider keys once");
                verify(&self.current(true).await?.client)?
            }
            Err(e) => return Err(e),
        };

        // Some providers keep email out of the ID token; ask the userinfo endpoint.
        if login.email.is_empty() {
            let http = self.http.clone();
            let fetch = move |req| {
                let http = http.clone();
                async move { http_client::execute(&http, req).await }
            };
            let info: CoreUserInfoClaims = d
                .client
                .user_info(token.access_token().clone(), None)
                .map_err(|e| OidcError::Config(e.to_string()))?
                .request_async(&fetch)
                .await
                .map_err(|e| OidcError::Provider(error_chain(&e)))?;
            // The userinfo `sub` must match the ID token's (OIDC Core 5.3.2).
            if info.subject().as_str() != login.subject {
                return Err(OidcError::InvalidToken("userinfo subject mismatch".into()));
            }
            login.email = info.email().map(|e| e.to_string()).ok_or(OidcError::MissingClaim("email"))?;
            login.email_verified = info.email_verified().unwrap_or(false);
        }
        Ok(login)
    }

    fn may_force_refresh(&self) -> bool {
        let mut last = self.last_forced.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.is_some_and(|t| t.elapsed() < MIN_FORCED_REFRESH) {
            return false;
        }
        *last = Some(Instant::now());
        true
    }

    /// RP-initiated logout URL (None if the provider has no end-session endpoint).
    pub async fn logout_url(&self, id_token: Option<&str>) -> Option<String> {
        let d = self.current(false).await.ok()?;
        let endpoint = d.end_session.clone()?;
        let mut req = LogoutRequest::from(endpoint)
            .set_client_id(ClientId::new(self.cfg.client_id.clone()))
            .set_post_logout_redirect_uri(PostLogoutRedirectUrl::new(self.cfg.post_logout_redirect_url.clone()).ok()?);
        if let Some(t) = id_token.and_then(|t| t.parse::<openidconnect::core::CoreIdToken>().ok()) {
            req = req.set_id_token_hint(&t);
        }
        Some(req.http_get_url().to_string())
    }
}

/// Read role names from a (signature-verified) ID token payload. Supports ZITADEL's object
/// form `{"role": {...}}` and plain arrays `["role"]`.
pub fn extract_roles(id_token: &str, claim: &str) -> Vec<String> {
    let Some(payload) = id_token.split('.').nth(1) else { return Vec::new() };
    let Ok(bytes) = URL_SAFE_NO_PAD.decode(payload) else { return Vec::new() };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return Vec::new() };
    match json.get(claim) {
        Some(serde_json::Value::Object(m)) => m.keys().cloned().collect(),
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        _ => Vec::new(),
    }
}

fn error_chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        s.push_str(": ");
        s.push_str(&c.to_string());
        cur = c.source();
    }
    s
}

/// Validate a post-login return path: same-origin relative path only (open-redirect guard).
pub fn safe_return_to(raw: Option<&str>) -> String {
    match raw {
        Some(p)
            if p.starts_with('/')
                && !p.starts_with("//")
                && !p.starts_with("/\\")
                && !p.contains("://")
                && !p.chars().any(|c| c.is_control())
                && p.len() <= 512 =>
        {
            p.to_string()
        }
        _ => "/dashboard".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_to_rejects_open_redirects() {
        assert_eq!(safe_return_to(Some("/org/acme/runs?x=1")), "/org/acme/runs?x=1");
        for bad in ["https://evil.example", "//evil.example", "/\\evil.example", "javascript:alert(1)", "/a\nb", ""] {
            assert_eq!(safe_return_to(Some(bad)), "/dashboard", "{bad:?}");
        }
        assert_eq!(safe_return_to(None), "/dashboard");
    }

    #[test]
    fn roles_from_zitadel_object_and_array_forms() {
        let enc = |v: serde_json::Value| format!("h.{}.s", URL_SAFE_NO_PAD.encode(v.to_string()));
        let z = enc(serde_json::json!({"urn:zitadel:iam:org:project:roles": {"system_admin": {"123": "org"}}}));
        assert_eq!(extract_roles(&z, "urn:zitadel:iam:org:project:roles"), vec!["system_admin"]);
        let a = enc(serde_json::json!({"roles": ["system_auditor", 5]}));
        assert_eq!(extract_roles(&a, "roles"), vec!["system_auditor"]);
        assert!(extract_roles("garbage", "roles").is_empty());
    }
}
