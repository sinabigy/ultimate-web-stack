//! Application services shared by handlers.

use std::sync::Arc;

use app_auth::{crypto::TokenCipher, idp_admin::IdentityAdmin, oidc::OidcProvider, service::ServiceTokenVerifier};
use app_authz::{Authorizer, rbac::Rbac};
use sqlx::PgPool;

pub struct Services {
    pub db: PgPool,
    pub oidc: Arc<OidcProvider>,
    /// Decision engine (RBAC or Cedar) for organisation and system checks.
    pub authz: Arc<dyn Authorizer>,
    /// Grant-time rules (invitations, credential scopes) shared by both engines.
    pub rbac: Rbac,
    pub cipher: TokenCipher,
    pub idp_admin: Arc<dyn IdentityAdmin>,
    pub service_tokens: Arc<ServiceTokenVerifier>,
    pub api_key_pepper: Vec<u8>,
}

#[async_trait::async_trait]
impl crate::health::HealthCheck for DbCheck {
    fn name(&self) -> &'static str {
        "postgres"
    }
    async fn check(&self) -> Result<(), String> {
        app_db::ping(&self.0).await.map_err(|e| e.to_string())
    }
}

pub struct DbCheck(pub PgPool);

/// The identity provider is needed for *new* logins only; existing sessions keep working,
/// so it is reported as degraded rather than failing readiness.
pub struct IdpCheck(pub Arc<OidcProvider>);

#[async_trait::async_trait]
impl crate::health::HealthCheck for IdpCheck {
    fn name(&self) -> &'static str {
        "identity_provider"
    }
    fn critical(&self) -> bool {
        false
    }
    async fn check(&self) -> Result<(), String> {
        self.0.check().await
    }
}
