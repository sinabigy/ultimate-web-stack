//! Provider-specific account administration, behind a vendor-neutral trait.
//!
//! Login is pure OIDC. Showing a user's passkeys/MFA factors, removing one, or resending a
//! verification email needs the provider's management API; those calls live here so business
//! logic never imports a vendor SDK. Providers without an API return `Unsupported`, and the UI
//! links to the provider's own account console instead.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export)]
pub struct AuthMethod {
    /// `passkey`, `totp`, `password`, `otp_email`, `otp_sms`, `idp_link`, ...
    pub kind: String,
    pub id: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default, ts_rs::TS)]
#[ts(export)]
pub struct SecurityOverview {
    pub methods: Vec<AuthMethod>,
    pub mfa_enabled: bool,
    #[ts(type = "number")]
    pub passkeys: usize,
    /// Where the user manages credentials when we cannot do it in-app.
    pub manage_url: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum IdpAdminError {
    #[error("not supported by the configured identity provider")]
    Unsupported,
    #[error("identity provider API error: {0}")]
    Api(String),
}

#[async_trait::async_trait]
pub trait IdentityAdmin: Send + Sync {
    /// Credentials registered for `subject` at the IdP.
    async fn security_overview(&self, subject: &str) -> Result<SecurityOverview, IdpAdminError>;
    /// Remove one passkey/factor. Caller must have verified ownership and recent authentication.
    async fn remove_method(&self, subject: &str, kind: &str, id: &str) -> Result<(), IdpAdminError>;
    async fn resend_email_verification(&self, subject: &str) -> Result<(), IdpAdminError>;
    /// Deactivate the identity at the IdP (account deletion workflow).
    async fn deactivate_user(&self, subject: &str) -> Result<(), IdpAdminError>;
}

/// Fallback for providers without a management API (or when no API token is configured).
pub struct ConsoleOnly {
    pub manage_url: Option<String>,
}

#[async_trait::async_trait]
impl IdentityAdmin for ConsoleOnly {
    async fn security_overview(&self, _subject: &str) -> Result<SecurityOverview, IdpAdminError> {
        Ok(SecurityOverview { manage_url: self.manage_url.clone(), ..Default::default() })
    }
    async fn remove_method(&self, _: &str, _: &str, _: &str) -> Result<(), IdpAdminError> {
        Err(IdpAdminError::Unsupported)
    }
    async fn resend_email_verification(&self, _: &str) -> Result<(), IdpAdminError> {
        Err(IdpAdminError::Unsupported)
    }
    async fn deactivate_user(&self, _: &str) -> Result<(), IdpAdminError> {
        Err(IdpAdminError::Unsupported)
    }
}

/// ZITADEL User Service v2 (`/v2/users/...`) via a service-account personal access token.
pub struct Zitadel {
    base: String,
    token: String,
    http: reqwest::Client,
    console_url: String,
}

impl Zitadel {
    pub fn new(issuer: &str, token: &str) -> Result<Self, IdpAdminError> {
        let http = crate::http_client::build(std::time::Duration::from_secs(10))
            .map_err(|e| IdpAdminError::Api(e.to_string()))?;
        let base = issuer.trim_end_matches('/').to_string();
        Ok(Self { console_url: format!("{base}/ui/console/users/me"), base, token: token.to_string(), http })
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, IdpAdminError> {
        let mut req = self.http.request(method, format!("{}{path}", self.base)).bearer_auth(&self.token);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.map_err(|e| IdpAdminError::Api(e.to_string()))?;
        let status = resp.status();
        let json = resp.json::<serde_json::Value>().await.unwrap_or(serde_json::Value::Null);
        if !status.is_success() {
            // Do not propagate response bodies (may contain internals); status is enough.
            return Err(IdpAdminError::Api(format!("HTTP {status}")));
        }
        Ok(json)
    }
}

/// ZITADEL authentication method type names → our neutral kinds.
fn zitadel_kind(t: &str) -> &str {
    match t {
        "AUTHENTICATION_METHOD_TYPE_PASSKEY" => "passkey",
        "AUTHENTICATION_METHOD_TYPE_TOTP" => "totp",
        "AUTHENTICATION_METHOD_TYPE_PASSWORD" => "password",
        "AUTHENTICATION_METHOD_TYPE_U2F" => "security_key",
        "AUTHENTICATION_METHOD_TYPE_OTP_EMAIL" => "otp_email",
        "AUTHENTICATION_METHOD_TYPE_OTP_SMS" => "otp_sms",
        "AUTHENTICATION_METHOD_TYPE_IDP" => "idp_link",
        _ => "other",
    }
}

#[async_trait::async_trait]
impl IdentityAdmin for Zitadel {
    async fn security_overview(&self, subject: &str) -> Result<SecurityOverview, IdpAdminError> {
        let v = self.call(reqwest::Method::GET, &format!("/v2/users/{subject}/authentication_methods"), None).await?;
        let methods: Vec<AuthMethod> = v["authMethodTypes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str())
                    .map(|t| AuthMethod { kind: zitadel_kind(t).to_string(), id: None, label: None })
                    .collect()
            })
            .unwrap_or_default();
        let passkeys = methods.iter().filter(|m| m.kind == "passkey").count();
        let mfa_enabled = methods
            .iter()
            .any(|m| matches!(m.kind.as_str(), "passkey" | "totp" | "security_key" | "otp_email" | "otp_sms"));
        Ok(SecurityOverview { methods, mfa_enabled, passkeys, manage_url: Some(self.console_url.clone()) })
    }

    async fn remove_method(&self, subject: &str, kind: &str, id: &str) -> Result<(), IdpAdminError> {
        let path = match kind {
            "passkey" => format!("/v2/users/{subject}/passkeys/{id}"),
            "totp" => format!("/v2/users/{subject}/totp"),
            "security_key" => format!("/v2/users/{subject}/u2f/{id}"),
            _ => return Err(IdpAdminError::Unsupported),
        };
        self.call(reqwest::Method::DELETE, &path, None).await.map(|_| ())
    }

    async fn resend_email_verification(&self, subject: &str) -> Result<(), IdpAdminError> {
        self.call(
            reqwest::Method::POST,
            &format!("/v2/users/{subject}/email/resend"),
            Some(serde_json::json!({"sendCode": {}})),
        )
        .await
        .map(|_| ())
    }

    async fn deactivate_user(&self, subject: &str) -> Result<(), IdpAdminError> {
        self.call(reqwest::Method::POST, &format!("/v2/users/{subject}/deactivate"), Some(serde_json::json!({})))
            .await
            .map(|_| ())
    }
}
