//! OpenAPI document for the integration surface: the operations a machine credential (an
//! organization API key or a registered service account) uses. Schemas derive from the same Rust
//! types the handlers return, so the document cannot describe a different shape.
//!
//! Browser-only operations (sign-in, session, account, invitations, credential management) are
//! not part of it; `tests/openapi.rs` lists them and fails when a new route is in neither list.
//! Served at `GET /api/v1/openapi.json`; a reviewed copy lives in `docs/api/openapi.json`.

use app_errors::Problem;
use axum::{http::header, response::IntoResponse};
use utoipa::{
    Modify, OpenApi,
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
};

use crate::routes::orgs;

/// Errors any integration operation can return, as RFC 9457 problem details.
#[derive(utoipa::IntoResponses)]
pub enum CommonErrors {
    /// Missing, invalid, expired or revoked credential. Never falls back to another method.
    #[response(status = 401, content_type = "application/problem+json")]
    Unauthenticated(Problem),
    /// The credential's scopes do not include the operation's permission.
    #[response(status = 403, content_type = "application/problem+json")]
    Forbidden(Problem),
    /// Unknown resource, or an organization the credential does not belong to.
    #[response(status = 404, content_type = "application/problem+json")]
    NotFound(Problem),
    /// Rate limited; wait for `Retry-After` seconds.
    #[response(status = 429, content_type = "application/problem+json")]
    RateLimited(Problem),
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Integration API",
        version = "v1",
        description = "Operations for API keys and service accounts. Authenticate with `Authorization: Bearer <credential>`; \
errors are RFC 9457 problem details; every response carries `x-request-id`. Examples: examples/api-clients.",
    ),
    paths(
        orgs::list_runs, orgs::create_run, orgs::get_run, orgs::delete_run, orgs::run_analytics,
        orgs::audit_log, orgs::list_members, orgs::list_teams, orgs::list_roles, orgs::permission_catalog,
    ),
    components(schemas(Problem, app_errors::FieldError)),
    modifiers(&BearerAuth),
    security(("bearer" = [])),
    tags((name = "runs"), (name = "organization"), (name = "audit")),
)]
pub struct IntegrationApi;

struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        let components = api.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "An organization API key, or a service account's JWT from the identity provider.",
                    ))
                    .build(),
            ),
        );
    }
}

/// The document as JSON (built once).
pub fn document() -> &'static str {
    static DOC: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DOC.get_or_init(|| IntegrationApi::openapi().to_pretty_json().unwrap_or_else(|_| "{}".into()))
}

/// `GET /api/v1/openapi.json`. Public: it describes the surface, not any data.
pub async fn serve() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], document())
}
