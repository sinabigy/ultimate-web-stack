//! Machine-to-machine authentication: OAuth 2.0 access tokens in JWT form (RFC 9068 style)
//! issued by the identity provider to service accounts (client credentials / JWT profile).
//!
//! Verified locally against the provider's JWKS: signature (asymmetric algorithms only, never
//! `none` or HMAC), issuer (exact), audience (one of `auth.service_audiences`), `exp`/`nbf`
//! with 60s leeway, and a present `sub`. Service principals are mapped to registered
//! `service_clients`; they are never users.

use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::http_client;

const JWKS_TTL: Duration = Duration::from_secs(3600);
const MIN_REFRESH: Duration = Duration::from_secs(30);
const ALLOWED: &[Algorithm] = &[Algorithm::RS256, Algorithm::PS256, Algorithm::ES256, Algorithm::EdDSA];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ServiceTokenError {
    #[error("malformed token")]
    Malformed,
    #[error("algorithm not allowed")]
    Algorithm,
    #[error("unknown signing key")]
    UnknownKey,
    #[error("invalid token: {0}")]
    Invalid(String),
    #[error("key set unavailable: {0}")]
    Unavailable(String),
    #[error("service tokens are not enabled")]
    Disabled,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceClaims {
    pub sub: String,
    #[serde(default)]
    pub client_id: Option<String>,
    /// Space-separated scopes (RFC 8693 / 9068).
    #[serde(default)]
    pub scope: Option<String>,
    pub exp: i64,
}

impl ServiceClaims {
    pub fn scopes(&self) -> Vec<&str> {
        self.scope.as_deref().map(|s| s.split_whitespace().collect()).unwrap_or_default()
    }
}

pub struct ServiceTokenVerifier {
    issuer: String,
    audiences: Vec<String>,
    http: reqwest::Client,
    jwks: RwLock<Option<(JwkSet, Instant)>>,
    last_refresh: std::sync::Mutex<Option<Instant>>,
    /// Test hook: fixed key set instead of discovery.
    fixed: Option<JwkSet>,
}

impl ServiceTokenVerifier {
    pub fn new(issuer: &str, audiences: Vec<String>) -> Result<Self, ServiceTokenError> {
        Ok(Self {
            issuer: issuer.to_string(),
            audiences,
            http: http_client::build(Duration::from_secs(10))
                .map_err(|e| ServiceTokenError::Unavailable(e.to_string()))?,
            jwks: RwLock::new(None),
            last_refresh: std::sync::Mutex::new(None),
            fixed: None,
        })
    }

    pub fn with_fixed_keys(issuer: &str, audiences: Vec<String>, keys: JwkSet) -> Result<Self, ServiceTokenError> {
        let mut v = Self::new(issuer, audiences)?;
        v.fixed = Some(keys);
        Ok(v)
    }

    pub fn enabled(&self) -> bool {
        !self.audiences.is_empty()
    }

    async fn fetch_jwks(&self) -> Result<JwkSet, ServiceTokenError> {
        if let Some(k) = &self.fixed {
            return Ok(k.clone());
        }
        let disco_url = format!("{}/.well-known/openid-configuration", self.issuer.trim_end_matches('/'));
        let disco: serde_json::Value = self
            .http
            .get(&disco_url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| ServiceTokenError::Unavailable(e.to_string()))?
            .json()
            .await
            .map_err(|e| ServiceTokenError::Unavailable(e.to_string()))?;
        let jwks_uri = disco["jwks_uri"].as_str().ok_or(ServiceTokenError::Unavailable("no jwks_uri".into()))?;
        self.http
            .get(jwks_uri)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| ServiceTokenError::Unavailable(e.to_string()))?
            .json::<JwkSet>()
            .await
            .map_err(|e| ServiceTokenError::Unavailable(e.to_string()))
    }

    async fn keys(&self, force: bool) -> Result<JwkSet, ServiceTokenError> {
        if !force
            && let Some((k, at)) = self.jwks.read().await.as_ref()
            && at.elapsed() < JWKS_TTL
        {
            return Ok(k.clone());
        }
        let k = self.fetch_jwks().await?;
        *self.jwks.write().await = Some((k.clone(), Instant::now()));
        Ok(k)
    }

    pub async fn verify(&self, token: &str) -> Result<ServiceClaims, ServiceTokenError> {
        if !self.enabled() {
            return Err(ServiceTokenError::Disabled);
        }
        let header = decode_header(token).map_err(|_| ServiceTokenError::Malformed)?;
        if !ALLOWED.contains(&header.alg) {
            return Err(ServiceTokenError::Algorithm);
        }
        let kid = header.kid.clone().ok_or(ServiceTokenError::UnknownKey)?;
        let mut keys = self.keys(false).await?;
        if keys.find(&kid).is_none() && self.may_refresh() {
            keys = self.keys(true).await?;
        }
        let jwk = keys.find(&kid).ok_or(ServiceTokenError::UnknownKey)?;
        let key = DecodingKey::from_jwk(jwk).map_err(|_| ServiceTokenError::UnknownKey)?;
        let mut v = Validation::new(header.alg);
        v.leeway = 60;
        v.set_issuer(&[&self.issuer]);
        v.set_audience(&self.audiences);
        v.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        v.validate_nbf = true;
        decode::<ServiceClaims>(token, &key, &v)
            .map(|d| d.claims)
            .map_err(|e| ServiceTokenError::Invalid(e.kind().clone().to_string_lossy()))
    }

    fn may_refresh(&self) -> bool {
        let mut last = self.last_refresh.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.is_some_and(|t| t.elapsed() < MIN_REFRESH) {
            return false;
        }
        *last = Some(Instant::now());
        true
    }
}

trait KindString {
    fn to_string_lossy(self) -> String;
}

impl KindString for jsonwebtoken::errors::ErrorKind {
    fn to_string_lossy(self) -> String {
        use jsonwebtoken::errors::ErrorKind as K;
        match self {
            K::ExpiredSignature => "expired".into(),
            K::InvalidIssuer => "wrong issuer".into(),
            K::InvalidAudience => "wrong audience".into(),
            K::ImmatureSignature => "not yet valid".into(),
            K::InvalidSignature => "bad signature".into(),
            K::MissingRequiredClaim(c) => format!("missing claim {c}"),
            other => format!("{other:?}"),
        }
    }
}
