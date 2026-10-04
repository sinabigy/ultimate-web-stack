//! Construct application services from configuration (used by the server binary and tests).

use std::sync::Arc;

use app_auth::{
    crypto::TokenCipher,
    idp_admin::{ConsoleOnly, IdentityAdmin, Zitadel},
    oidc::OidcProvider,
    service::ServiceTokenVerifier,
};
use app_authz::{Authorizer, rbac::Rbac};
use app_config::{AppConfig, AuthorizationEngine, IdentityProviderKind};
use sqlx::PgPool;

use crate::services::Services;

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("{0}")]
    Config(String),
}

pub fn build_services(cfg: &AppConfig, db: PgPool) -> Result<Services, BootstrapError> {
    let a = &cfg.auth;
    let oidc = Arc::new(OidcProvider::new(a.clone()).map_err(|e| BootstrapError::Config(e.to_string()))?);

    let authz: Arc<dyn Authorizer> = match cfg.authorization.engine {
        AuthorizationEngine::Rbac => Arc::new(Rbac),
        #[cfg(feature = "cedar")]
        AuthorizationEngine::Cedar => Arc::new(
            app_authz::cedar::CedarAuthorizer::with_policy_dir(&cfg.authorization.cedar_policy_dir)
                .map_err(|e| BootstrapError::Config(e.to_string()))?,
        ),
        #[cfg(not(feature = "cedar"))]
        AuthorizationEngine::Cedar => {
            return Err(BootstrapError::Config(
                "authorization.engine = cedar requires building with the `cedar` feature".into(),
            ));
        }
    };

    let cipher = if a.token_encryption_key.is_empty() {
        tracing::warn!(
            "auth.token_encryption_key not set: using an ephemeral key (development only; logout hints lost on restart)"
        );
        TokenCipher::ephemeral()
    } else {
        TokenCipher::from_base64_key(a.token_encryption_key.expose())
            .map_err(|e| BootstrapError::Config(e.to_string()))?
    };

    let api_key_pepper = if a.api_key_pepper.is_empty() {
        tracing::warn!("auth.api_key_pepper not set: using a fixed development pepper (never in production)");
        b"development-only-api-key-pepper".to_vec()
    } else {
        a.api_key_pepper.expose().as_bytes().to_vec()
    };

    let idp_admin: Arc<dyn IdentityAdmin> = match (a.provider, a.zitadel_api_token.is_empty()) {
        (IdentityProviderKind::Zitadel, false) => Arc::new(
            Zitadel::new(&a.issuer_url, a.zitadel_api_token.expose())
                .map_err(|e| BootstrapError::Config(e.to_string()))?,
        ),
        (IdentityProviderKind::Zitadel, true) => Arc::new(ConsoleOnly {
            manage_url: Some(format!("{}/ui/console/users/me", a.issuer_url.trim_end_matches('/'))),
        }),
        (IdentityProviderKind::Oidc, _) => Arc::new(ConsoleOnly {
            manage_url: (!a.account_console_url.is_empty()).then(|| a.account_console_url.clone()),
        }),
    };

    let service_tokens = Arc::new(
        ServiceTokenVerifier::new(&a.issuer_url, a.service_audiences.clone())
            .map_err(|e| BootstrapError::Config(e.to_string()))?,
    );

    Ok(Services { db, oidc, authz, rbac: Rbac, cipher, idp_admin, service_tokens, api_key_pepper })
}
