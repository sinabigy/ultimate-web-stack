//! Application configuration.
//!
//! Layering (later wins): compiled defaults → optional TOML file (`APP_CONFIG` or
//! `config/app.toml`) → environment variables prefixed `APP__`, with `__` separating
//! nesting levels (`APP__DATABASE__URL`, `APP__HTTP__PORT`).
//!
//! Every optional module has an `enabled` flag here; the defaults describe the **core**
//! profile (PostgreSQL only). Secrets (database URL, session secret) come from the
//! environment and are never logged: `Debug` for [`Secret`] is redacted.

use std::{fmt, net::IpAddr, path::PathBuf, time::Duration};

use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("configuration could not be loaded: {0}")]
    Load(#[from] Box<figment::Error>),
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

/// Environment values are parsed by type, so `APP__AUTH__CLIENT_ID=393614044919037955` arrives
/// as an integer. Identifiers and secrets must accept any scalar and keep its exact text.
mod lenient {
    use serde::{Deserialize, Deserializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Scalar {
        // Order matters: integers before floats so large ids never lose precision through f64.
        S(String),
        U(u64),
        I(i64),
        F(f64),
        B(bool),
    }

    impl Scalar {
        fn into_string(self) -> String {
            match self {
                Scalar::S(s) => s,
                Scalar::I(i) => i.to_string(),
                Scalar::U(u) => u.to_string(),
                Scalar::F(f) => f.to_string(),
                Scalar::B(b) => b.to_string(),
            }
        }
    }

    pub fn string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        Ok(Scalar::deserialize(d)?.into_string())
    }

    pub fn strings<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
        Ok(Vec::<Scalar>::deserialize(d)?.into_iter().map(Scalar::into_string).collect())
    }
}

/// A string that never appears in logs or debug output.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct Secret(#[serde(deserialize_with = "lenient::string")] String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_empty() { "Secret(<empty>)" } else { "Secret(<redacted>)" })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    /// Deployment environment name: `development`, `test`, `production`, ...
    pub environment: String,
    pub http: HttpConfig,
    pub log: LogConfig,
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
    pub tenancy: TenancyConfig,
    pub authorization: AuthorizationConfig,
    pub admin: AdminConfig,
    pub cache: CacheConfig,
    pub rate_limit: RateLimitConfig,
    pub messaging: MessagingConfig,
    pub analytics: AnalyticsConfig,
    pub jobs: JobsConfig,
    pub providers: ProvidersConfig,
    pub telemetry: TelemetryConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            environment: "development".into(),
            http: HttpConfig::default(),
            log: LogConfig::default(),
            database: DatabaseConfig::default(),
            auth: AuthConfig::default(),
            tenancy: TenancyConfig::default(),
            authorization: AuthorizationConfig::default(),
            admin: AdminConfig::default(),
            cache: CacheConfig::default(),
            rate_limit: RateLimitConfig::default(),
            messaging: MessagingConfig::default(),
            analytics: AnalyticsConfig::default(),
            jobs: JobsConfig::default(),
            providers: ProvidersConfig::default(),
            telemetry: TelemetryConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct HttpConfig {
    pub host: IpAddr,
    pub port: u16,
    /// Per-request timeout for ordinary routes (streaming routes are exempt).
    pub request_timeout_ms: u64,
    /// Maximum request body size in bytes.
    pub body_limit_bytes: usize,
    /// Global in-flight request cap; beyond it requests are shed with 503 instead of queueing.
    pub max_inflight: usize,
    /// After SIGTERM: report not-ready for this long so load balancers stop routing here,
    /// then stop accepting and drain.
    pub shutdown_drain_ms: u64,
    /// Hard deadline for in-flight requests to finish after draining starts.
    pub shutdown_timeout_ms: u64,
    /// Exact allowed CORS origins. Empty = same-origin only (recommended).
    pub cors_origins: Vec<String>,
    /// Serve the built frontend from this directory (simple production profile). None = API only.
    pub static_dir: Option<PathBuf>,
    /// Emit `Strict-Transport-Security`. Enable only when every request reaches users over TLS.
    pub hsts: bool,
    /// Expose `/bench/*` endpoints used by the benchmark suite.
    pub bench_endpoints: bool,
    /// Trust `X-Forwarded-For`/`CF-Connecting-IP` for client IPs (only behind a trusted proxy).
    pub trust_forwarded_for: bool,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            host: IpAddr::from([127, 0, 0, 1]),
            port: 8080,
            request_timeout_ms: 15_000,
            body_limit_bytes: 1024 * 1024,
            max_inflight: 10_000,
            shutdown_drain_ms: 0,
            shutdown_timeout_ms: 20_000,
            cors_origins: Vec::new(),
            static_dir: None,
            hsts: false,
            bench_endpoints: false,
            trust_forwarded_for: false,
        }
    }
}

impl HttpConfig {
    pub fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.request_timeout_ms)
    }
    pub fn shutdown_drain(&self) -> Duration {
        Duration::from_millis(self.shutdown_drain_ms)
    }
    pub fn shutdown_timeout(&self) -> Duration {
        Duration::from_millis(self.shutdown_timeout_ms)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Pretty,
    Json,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LogConfig {
    /// `tracing_subscriber::EnvFilter` directive; `RUST_LOG` overrides it when set.
    pub filter: String,
    pub format: LogFormat,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self { filter: "info,app=debug,tower_http=info,sqlx=warn".into(), format: LogFormat::Pretty }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DatabaseConfig {
    pub url: Secret,
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout_ms: u64,
    pub idle_timeout_ms: u64,
    /// Server-side `statement_timeout` applied to every pooled connection.
    pub statement_timeout_ms: u64,
    /// Statements slower than this are logged at WARN.
    pub slow_statement_ms: u64,
    pub migrate_on_start: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: Secret::default(),
            max_connections: 32,
            min_connections: 2,
            acquire_timeout_ms: 3_000,
            idle_timeout_ms: 600_000,
            statement_timeout_ms: 10_000,
            slow_statement_ms: 200,
            migrate_on_start: false,
        }
    }
}

/// Which identity provider adapter is used for provider-specific *administration* features
/// (listing passkeys/MFA, resending verification). Login itself is always standard OIDC.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum IdentityProviderKind {
    /// ZITADEL (self-hosted or Cloud): OIDC + optional management API via a service account.
    Zitadel,
    /// Any standards-compliant OIDC provider (also used for the local mock IdP in tests).
    Oidc,
}

/// Auth feature bundle chosen at generation time. Purely descriptive at runtime except
/// where noted; individual flags below are authoritative.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthProfile {
    /// Login, registration, account center.
    Basic,
    /// Basic + passkey-first onboarding + social login.
    Consumer,
    /// Consumer + organisations, invitations, org roles, org dashboard.
    B2b,
    /// B2B + enterprise SSO (OIDC/SAML federation) + advanced authorization.
    Enterprise,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SameSite {
    Lax,
    Strict,
}

/// Login methods offered on the application's login page. A method is shown only when
/// enabled *and* configured (social methods need an IdP id at the identity provider).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct LoginMethods {
    pub passkey: bool,
    pub password: bool,
    /// Social / enterprise identity providers: name → IdP id at the identity provider
    /// (ZITADEL: the IdP id used in the `urn:zitadel:iam:org:idp:id:<id>` scope).
    pub social: std::collections::BTreeMap<String, String>,
    /// Show "Continue with SSO" (enterprise federation, resolved by email domain at the IdP).
    pub enterprise_sso: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AuthConfig {
    pub provider: IdentityProviderKind,
    pub profile: AuthProfile,
    /// OIDC issuer. Discovery is fetched from `<issuer>/.well-known/openid-configuration`.
    pub issuer_url: String,
    #[serde(deserialize_with = "lenient::string")]
    pub client_id: String,
    /// Confidential client secret. Empty = public client (PKCE only). PKCE is always used.
    pub client_secret: Secret,
    /// Absolute URL of this application's `/auth/callback` (must be registered at the IdP).
    pub redirect_url: String,
    /// Where the IdP returns the browser after RP-initiated logout.
    pub post_logout_redirect_url: String,
    /// Public origin of the web app; `return_to` targets must be same-origin relative paths.
    pub public_origin: String,
    pub scopes: Vec<String>,
    pub methods: LoginMethods,
    pub allow_registration: bool,
    /// Account console of a generic OIDC provider (used for passkey/MFA management links).
    pub account_console_url: String,

    pub cookie_name: String,
    /// `Secure` cookie attribute (and `__Host-` prefix). Must be true in production.
    pub cookie_secure: bool,
    pub cookie_same_site: SameSite,
    /// Absolute session lifetime: re-authentication is required after this, regardless of activity.
    pub session_absolute_ttl_minutes: u64,
    /// Idle timeout: sessions unused for this long expire.
    pub session_idle_ttl_minutes: u64,
    /// Rotate the session token at most this often while in use.
    pub session_rotate_minutes: u64,
    /// "Recent authentication" window for sensitive actions (account deletion, step-up).
    pub reauth_window_minutes: u64,
    pub store_client_ip: bool,
    pub store_user_agent: bool,
    /// 32-byte key (base64) used to encrypt IdP tokens at rest (AES-256-GCM).
    pub token_encryption_key: Secret,
    /// Server-side pepper for API key hashes (HMAC-SHA256).
    pub api_key_pepper: Secret,
    /// API keys look like `<prefix>_<env>_<id>_<secret>`.
    pub api_key_prefix: String,
    /// Extra `aud` values allowed in ID tokens besides `client_id` (OIDC Core 3.1.3.7 requires
    /// rejecting untrusted extra audiences). ZITADEL adds the project id here.
    #[serde(deserialize_with = "lenient::strings")]
    pub id_token_trusted_audiences: Vec<String>,
    /// Accepted `aud` values for machine-to-machine JWT bearer tokens. Empty disables M2M JWTs.
    #[serde(deserialize_with = "lenient::strings")]
    pub service_audiences: Vec<String>,
    /// System-admin endpoints require a session whose authentication used MFA.
    pub require_mfa_for_system_admin: bool,
    /// Email addresses granted the `system_admin` level on first login (bootstrap only).
    #[serde(deserialize_with = "lenient::strings")]
    pub bootstrap_system_admins: Vec<String>,
    /// Take the system trust level from an ID-token claim at every login (IdP is the source
    /// of truth for platform operators). Organisation roles always stay application-owned.
    pub system_roles_from_idp: bool,
    /// Claim holding granted project roles. ZITADEL: `urn:zitadel:iam:org:project:roles`
    /// (an object keyed by role name); generic providers: an array of role names.
    pub system_role_claim: String,
    /// ZITADEL management API access (service account personal access token). Optional.
    pub zitadel_api_token: Secret,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            provider: IdentityProviderKind::Zitadel,
            profile: AuthProfile::B2b,
            issuer_url: "http://localhost:8081".into(),
            client_id: String::new(),
            client_secret: Secret::default(),
            redirect_url: "http://localhost:5190/auth/callback".into(),
            post_logout_redirect_url: "http://localhost:5190/login".into(),
            public_origin: "http://localhost:5190".into(),
            scopes: vec!["openid".into(), "profile".into(), "email".into()],
            methods: LoginMethods { passkey: true, password: true, ..Default::default() },
            allow_registration: true,
            account_console_url: String::new(),
            cookie_name: "app_session".into(),
            cookie_secure: false,
            cookie_same_site: SameSite::Lax,
            session_absolute_ttl_minutes: 7 * 24 * 60,
            session_idle_ttl_minutes: 12 * 60,
            session_rotate_minutes: 60,
            reauth_window_minutes: 10,
            store_client_ip: true,
            store_user_agent: true,
            token_encryption_key: Secret::default(),
            api_key_pepper: Secret::default(),
            api_key_prefix: "app".into(),
            id_token_trusted_audiences: Vec::new(),
            service_audiences: Vec::new(),
            require_mfa_for_system_admin: true,
            bootstrap_system_admins: Vec::new(),
            system_roles_from_idp: false,
            system_role_claim: "urn:zitadel:iam:org:project:roles".into(),
            zitadel_api_token: Secret::default(),
        }
    }
}

impl AuthConfig {
    /// Cookie name actually used: `__Host-` prefixed when Secure (binds it to this exact host,
    /// path `/`, no Domain attribute; browsers enforce this).
    pub fn effective_cookie_name(&self) -> String {
        if self.cookie_secure { format!("__Host-{}", self.cookie_name) } else { self.cookie_name.clone() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TenancyConfig {
    /// Multi-organisation UI and APIs. When false every user still gets a personal
    /// organisation (data stays tenant-scoped, so enabling this later needs no migration).
    pub organizations: bool,
    /// Users may create additional organisations.
    pub allow_org_creation: bool,
    pub invitation_ttl_hours: u64,
}

impl Default for TenancyConfig {
    fn default() -> Self {
        Self { organizations: true, allow_org_creation: true, invitation_ttl_hours: 72 }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthorizationEngine {
    /// Role → permission mapping + ownership + tenant membership (default).
    Rbac,
    /// Cedar policies (RBAC + ABAC + context). Requires the `cedar` build feature.
    Cedar,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AuthorizationConfig {
    pub engine: AuthorizationEngine,
    /// Directory containing `*.cedar` policies (Cedar engine only).
    pub cedar_policy_dir: PathBuf,
}

impl Default for AuthorizationConfig {
    fn default() -> Self {
        Self { engine: AuthorizationEngine::Rbac, cedar_policy_dir: PathBuf::from("policies") }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AdminConfig {
    /// System administration APIs and UI.
    pub enabled: bool,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CacheBackend {
    /// In-process TTL cache. Zero infrastructure; per-instance.
    Memory,
    /// Redis or any RESP-compatible server (Dragonfly, Valkey). Shared across instances.
    Redis,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CacheConfig {
    pub backend: CacheBackend,
    pub redis_url: Secret,
    /// Key namespace prefix: `<namespace>:<domain>:<key>`.
    pub namespace: String,
    pub default_ttl_ms: u64,
    pub memory_max_entries: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            backend: CacheBackend::Memory,
            redis_url: Secret::default(),
            namespace: "app".into(),
            default_ttl_ms: 30_000,
            memory_max_entries: 100_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RateLimitConfig {
    pub enabled: bool,
    /// Sustained requests per second allowed per client key (IP or user).
    pub per_client_rps: f64,
    pub burst: u32,
    /// `memory` (per instance) or `redis` (shared; requires cache.redis_url).
    pub backend: CacheBackend,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self { enabled: true, per_client_rps: 50.0, burst: 100, backend: CacheBackend::Memory }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MessagingConfig {
    /// NATS/JetStream. Off in the core profile: jobs use PostgreSQL and events stay in-process.
    pub enabled: bool,
    pub nats_url: String,
    pub stream: String,
    /// Deliveries before a message is dead-lettered.
    pub max_deliver: i64,
    pub ack_wait_ms: u64,
}

impl Default for MessagingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            nats_url: "nats://127.0.0.1:4222".into(),
            stream: "APP_JOBS".into(),
            max_deliver: 5,
            ack_wait_ms: 30_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AnalyticsConfig {
    /// ClickHouse event analytics. Off in the core profile (events are only logged at debug).
    pub enabled: bool,
    pub clickhouse_url: String,
    pub database: String,
    pub user: String,
    pub password: Secret,
    pub batch_size: usize,
    pub flush_interval_ms: u64,
    /// Bounded in-memory buffer; when full, events are dropped and counted (never block requests).
    pub buffer_capacity: usize,
}

impl Default for AnalyticsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            clickhouse_url: "http://127.0.0.1:8123".into(),
            database: "app".into(),
            user: "default".into(),
            password: Secret::default(),
            batch_size: 10_000,
            flush_interval_ms: 1_000,
            buffer_capacity: 100_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct JobsConfig {
    /// Run job workers inside the API process (simple deployments). Large deployments run `worker`.
    pub run_in_process: bool,
    pub concurrency: usize,
    pub poll_interval_ms: u64,
    pub max_attempts: i32,
}

impl Default for JobsConfig {
    fn default() -> Self {
        Self { run_in_process: true, concurrency: 8, poll_interval_ms: 1_000, max_attempts: 5 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ProvidersConfig {
    /// Outbound provider definitions, keyed by provider name.
    pub definitions: std::collections::BTreeMap<String, ProviderDefinition>,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        let mut definitions = std::collections::BTreeMap::new();
        definitions.insert("simulated".into(), ProviderDefinition::default());
        Self { definitions }
    }
}

/// Configuration for one outbound provider. See `app-networking` for semantics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderDefinition {
    pub base_url: String,
    /// Bearer token or API key; read from the environment, never committed.
    pub api_key: Secret,
    pub initial_concurrency: usize,
    pub min_concurrency: usize,
    pub max_concurrency: usize,
    /// Sustained request rate allowed by the provider contract (0 = unlimited).
    pub requests_per_second: f64,
    pub burst: u32,
    /// Token budget per minute (LLM-style providers; 0 = unlimited).
    pub tokens_per_minute: u64,
    pub request_timeout_ms: u64,
    pub connect_timeout_ms: u64,
    pub max_retries: u32,
    pub http2_prior_knowledge: bool,
}

impl Default for ProviderDefinition {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:9090".into(),
            api_key: Secret::default(),
            initial_concurrency: 32,
            min_concurrency: 2,
            max_concurrency: 512,
            requests_per_second: 0.0,
            burst: 0,
            tokens_per_minute: 0,
            request_timeout_ms: 10_000,
            connect_timeout_ms: 2_000,
            max_retries: 3,
            http2_prior_knowledge: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TelemetryConfig {
    pub service_name: String,
    /// OTLP endpoint (e.g. `http://127.0.0.1:4318`). Empty disables trace export.
    pub otlp_endpoint: String,
    /// Fraction of traces sampled when exporting (parent-based).
    pub trace_sample_ratio: f64,
    /// Serve Prometheus metrics at `/metrics`.
    pub metrics: bool,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self { service_name: "app".into(), otlp_endpoint: String::new(), trace_sample_ratio: 1.0, metrics: true }
    }
}

fn origin_of(url: &str) -> Option<&str> {
    let rest_start = url.find("://")? + 3;
    let end = url[rest_start..].find('/').map_or(url.len(), |i| rest_start + i);
    Some(&url[..end])
}

impl AppConfig {
    /// Identity configuration checks that can be made without network access.
    pub fn validate_auth(&self) -> Result<(), ConfigError> {
        let a = &self.auth;
        let bad = |m: String| Err(ConfigError::Invalid(m));
        for (k, v) in [
            ("issuer_url", &a.issuer_url),
            ("redirect_url", &a.redirect_url),
            ("post_logout_redirect_url", &a.post_logout_redirect_url),
            ("public_origin", &a.public_origin),
        ] {
            if !(v.starts_with("https://") || v.starts_with("http://")) || v.contains('#') {
                return bad(format!("auth.{k} must be an absolute http(s) URL without fragment"));
            }
        }
        if origin_of(&a.public_origin) != Some(a.public_origin.trim_end_matches('/')) {
            return bad("auth.public_origin must be an origin (scheme://host[:port]) without a path".into());
        }
        for (k, v) in [("redirect_url", &a.redirect_url), ("post_logout_redirect_url", &a.post_logout_redirect_url)] {
            if origin_of(v) != origin_of(&a.public_origin) {
                return bad(format!("auth.{k} must be on auth.public_origin (BFF cookies are same-origin)"));
            }
        }
        if !a.redirect_url.ends_with("/auth/callback") {
            return bad("auth.redirect_url must point at the BFF callback path /auth/callback".into());
        }
        if !a.scopes.iter().any(|s| s == "openid") {
            return bad("auth.scopes must include openid".into());
        }
        if a.cookie_secure
            && !a.public_origin.starts_with("https://")
            && !a.public_origin.starts_with("http://localhost")
        {
            return bad("auth.cookie_secure requires an https public_origin (or localhost)".into());
        }
        if a.cookie_name.is_empty() || !a.cookie_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return bad("auth.cookie_name must be a simple token".into());
        }
        if a.session_idle_ttl_minutes == 0
            || a.session_idle_ttl_minutes > a.session_absolute_ttl_minutes
            || a.session_rotate_minutes == 0
        {
            return bad(
                "auth: require 0 < session_idle_ttl_minutes <= session_absolute_ttl_minutes and rotate > 0".into()
            );
        }
        if !a.api_key_prefix.chars().all(|c| c.is_ascii_lowercase())
            || a.api_key_prefix.is_empty()
            || a.api_key_prefix.len() > 8
        {
            return bad("auth.api_key_prefix must be 1-8 lowercase letters".into());
        }
        Ok(())
    }

    /// Load from the standard layers. `APP_CONFIG` names an explicit TOML file.
    pub fn load() -> Result<Self, ConfigError> {
        let file = std::env::var("APP_CONFIG").unwrap_or_else(|_| "config/app.toml".into());
        Self::from_figment(
            Figment::from(Serialized::defaults(AppConfig::default()))
                .merge(Toml::file(file))
                .merge(Env::prefixed("APP__").split("__")),
        )
    }

    pub fn from_figment(figment: Figment) -> Result<Self, ConfigError> {
        let cfg: AppConfig = figment.extract().map_err(Box::new)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn is_production(&self) -> bool {
        self.environment == "production"
    }

    /// Reject configurations that would be unsafe or nonsensical, at startup rather than at 3am.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let bad = |m: &str| Err(ConfigError::Invalid(m.to_string()));
        if self.http.body_limit_bytes == 0 {
            return bad("http.body_limit_bytes must be > 0");
        }
        if self.http.max_inflight == 0 {
            return bad("http.max_inflight must be > 0");
        }
        if self.database.max_connections == 0 || self.database.min_connections > self.database.max_connections {
            return bad("database: require 0 < min_connections <= max_connections");
        }
        self.validate_auth()?;
        if self.is_production() {
            if !self.auth.cookie_secure {
                return bad("auth.cookie_secure must be true in production");
            }
            for (k, v) in [
                ("auth.issuer_url", &self.auth.issuer_url),
                ("auth.redirect_url", &self.auth.redirect_url),
                ("auth.public_origin", &self.auth.public_origin),
            ] {
                if !v.starts_with("https://") {
                    return Err(ConfigError::Invalid(format!("{k} must use https in production")));
                }
            }
            if self.auth.token_encryption_key.is_empty() || self.auth.api_key_pepper.is_empty() {
                return bad("auth.token_encryption_key and auth.api_key_pepper are required in production");
            }
            if self.http.bench_endpoints {
                return bad("http.bench_endpoints must be false in production");
            }
        }
        if (self.cache.backend == CacheBackend::Redis || self.rate_limit.backend == CacheBackend::Redis)
            && self.cache.redis_url.is_empty()
        {
            return bad("cache.redis_url is required when a Redis backend is selected");
        }
        if self.rate_limit.enabled && (self.rate_limit.per_client_rps <= 0.0 || self.rate_limit.burst == 0) {
            return bad("rate_limit: per_client_rps and burst must be > 0 when enabled");
        }
        if !(0.0..=1.0).contains(&self.telemetry.trace_sample_ratio) {
            return bad("telemetry.trace_sample_ratio must be within [0, 1]");
        }
        for (name, p) in &self.providers.definitions {
            if p.min_concurrency == 0
                || p.min_concurrency > p.initial_concurrency
                || p.initial_concurrency > p.max_concurrency
            {
                return Err(ConfigError::Invalid(format!(
                    "providers.{name}: require 0 < min_concurrency <= initial_concurrency <= max_concurrency"
                )));
            }
            if p.requests_per_second < 0.0 {
                return Err(ConfigError::Invalid(format!("providers.{name}: requests_per_second must be >= 0")));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::result_large_err)] // figment::Jail's closure signature returns figment::Error
mod tests {
    use super::*;
    use figment::Jail;

    fn load(f: Figment) -> Result<AppConfig, ConfigError> {
        AppConfig::from_figment(Figment::from(Serialized::defaults(AppConfig::default())).merge(f))
    }

    #[test]
    fn defaults_are_valid_core_profile() {
        let c = AppConfig::default();
        c.validate().expect("defaults must validate");
        assert!(!c.messaging.enabled && !c.analytics.enabled);
        assert_eq!(c.cache.backend, CacheBackend::Memory);
    }

    #[test]
    fn env_overrides_nested_values() {
        Jail::expect_with(|jail| {
            jail.set_env("APP__HTTP__PORT", "9999");
            jail.set_env("APP__DATABASE__URL", "postgres://u:p@h/db");
            jail.set_env("APP__MESSAGING__ENABLED", "true");
            let c = load(Figment::new().merge(Env::prefixed("APP__").split("__"))).expect("load");
            assert_eq!(c.http.port, 9999);
            assert_eq!(c.database.url.expose(), "postgres://u:p@h/db");
            assert!(c.messaging.enabled);
            Ok(())
        });
    }

    #[test]
    fn numeric_identifiers_and_secrets_from_env_stay_strings() {
        Jail::expect_with(|jail| {
            jail.set_env("APP__AUTH__CLIENT_ID", "393614044919037955");
            jail.set_env("APP__AUTH__SERVICE_AUDIENCES", "[393614044835086339, app-api]");
            jail.set_env("APP__AUTH__CLIENT_SECRET", "12345678901234567890");
            let c = load(Figment::new().merge(Env::prefixed("APP__").split("__"))).expect("load");
            assert_eq!(c.auth.client_id, "393614044919037955");
            assert_eq!(c.auth.service_audiences, vec!["393614044835086339", "app-api"]);
            assert_eq!(c.auth.client_secret.expose(), "12345678901234567890");
            Ok(())
        });
    }

    #[test]
    fn toml_file_layer_and_unknown_keys_rejected() {
        Jail::expect_with(|jail| {
            jail.create_file("a.toml", "[http]\nport = 7000\n")?;
            assert_eq!(load(Figment::new().merge(Toml::file("a.toml"))).expect("load").http.port, 7000);
            jail.create_file("b.toml", "[http]\nprot = 7000\n")?;
            assert!(load(Figment::new().merge(Toml::file("b.toml"))).is_err(), "typo must not be ignored");
            Ok(())
        });
    }

    fn production() -> AppConfig {
        let mut c = AppConfig { environment: "production".into(), ..Default::default() };
        c.auth.cookie_secure = true;
        c.auth.issuer_url = "https://id.example.com".into();
        c.auth.public_origin = "https://app.example.com".into();
        c.auth.redirect_url = "https://app.example.com/auth/callback".into();
        c.auth.post_logout_redirect_url = "https://app.example.com/login".into();
        c.auth.token_encryption_key = Secret::new("k");
        c.auth.api_key_pepper = Secret::new("p");
        c
    }

    #[test]
    fn production_requires_secure_cookies_https_secrets_and_no_bench_endpoints() {
        production().validate().expect("secure prod config");
        let mut c = production();
        c.auth.cookie_secure = false;
        assert!(c.validate().is_err());
        let mut c = production();
        c.auth.issuer_url = "http://id.example.com".into();
        assert!(c.validate().is_err());
        let mut c = production();
        c.auth.api_key_pepper = Secret::default();
        assert!(c.validate().is_err());
        let mut c = production();
        c.http.bench_endpoints = true;
        assert!(c.validate().is_err());
    }

    #[test]
    fn redirect_uris_must_be_same_origin_bff_callback() {
        let mut c = AppConfig::default();
        c.auth.redirect_url = "http://evil.example/auth/callback".into();
        assert!(c.validate().is_err(), "cross-origin redirect");
        let mut c = AppConfig::default();
        c.auth.redirect_url = "http://localhost:5190/callback".into();
        assert!(c.validate().is_err(), "wrong path");
        let mut c = AppConfig::default();
        c.auth.public_origin = "http://localhost:5190/app".into();
        assert!(c.validate().is_err(), "origin with path");
        let mut c = AppConfig::default();
        c.auth.scopes = vec!["email".into()];
        assert!(c.validate().is_err(), "openid scope required");
    }

    #[test]
    fn secure_cookies_use_host_prefix() {
        let mut a = AuthConfig::default();
        assert_eq!(a.effective_cookie_name(), "app_session");
        a.cookie_secure = true;
        assert_eq!(a.effective_cookie_name(), "__Host-app_session");
    }

    #[test]
    fn redis_backend_requires_url() {
        let mut c = AppConfig::default();
        c.cache.backend = CacheBackend::Redis;
        assert!(c.validate().is_err());
        c.cache.redis_url = Secret::new("redis://127.0.0.1:6379");
        c.validate().expect("valid with url");
    }

    #[test]
    fn provider_concurrency_bounds_validated() {
        let mut c = AppConfig::default();
        if let Some(p) = c.providers.definitions.get_mut("simulated") {
            p.min_concurrency = 100;
        }
        assert!(c.validate().is_err());
    }

    #[test]
    fn secrets_are_redacted_in_debug() {
        let s = Secret::new("hunter2");
        assert!(!format!("{s:?}").contains("hunter2"));
        let mut c = AppConfig::default();
        c.database.url = Secret::new("postgres://user:hunter2@db/app");
        assert!(!format!("{c:?}").contains("hunter2"));
    }
}
