pub mod account;
pub mod admin;
pub mod bench;
pub mod ops;
pub mod orgs;
pub mod realtime;

use axum::{
    Router,
    routing::{delete, get, patch, post, put},
};

use crate::{auth::routes as auth, state::AppState};

/// `/auth/*` browser endpoints (BFF).
pub fn auth_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", get(auth::login))
        .route("/auth/register", get(auth::register))
        .route("/auth/reauth", get(auth::reauth))
        .route("/auth/callback", get(auth::callback))
        .route("/auth/logout", post(auth::logout))
}

/// `/api/v1/*` application API.
pub fn api_routes(admin_enabled: bool) -> Router<AppState> {
    let mut r = Router::new()
        .route("/api/v1/session", get(auth::session))
        .route("/api/v1/dashboard", get(account::dashboard))
        .route("/api/v1/account/profile", get(account::get_profile).patch(account::update_profile))
        .route("/api/v1/account/preferences", put(account::update_preferences))
        .route("/api/v1/account/security", get(account::security))
        .route("/api/v1/account/security/methods/{kind}/{id}", delete(account::remove_method))
        .route("/api/v1/account/security/verify-email", post(account::resend_verification))
        .route("/api/v1/account/sessions", get(account::list_sessions))
        .route("/api/v1/account/sessions/{id}", delete(account::revoke_session))
        .route("/api/v1/account/sessions/revoke-all", post(account::revoke_all_sessions))
        .route("/api/v1/account/activity", get(account::activity))
        .route("/api/v1/account/delete", post(account::delete_account))
        .route("/api/v1/notifications", get(account::list_notifications))
        .route("/api/v1/notifications/read-all", post(account::read_all_notifications))
        .route("/api/v1/notifications/{id}/read", post(account::read_notification))
        .route("/api/v1/permissions", get(orgs::permission_catalog))
        .route("/api/v1/invitations/{token}", get(orgs::invitation_preview))
        .route("/api/v1/invitations/{token}/accept", post(orgs::accept_invitation))
        .route("/api/v1/orgs", get(orgs::list_mine).post(orgs::create))
        .route("/api/v1/orgs/{slug}", get(orgs::get).patch(orgs::update).delete(orgs::delete))
        .route("/api/v1/orgs/{slug}/overview", get(orgs::overview))
        .route("/api/v1/orgs/{slug}/leave", post(orgs::leave))
        .route("/api/v1/orgs/{slug}/members", get(orgs::list_members))
        .route("/api/v1/orgs/{slug}/members/{user_id}", patch(orgs::change_member_role).delete(orgs::remove_member))
        .route("/api/v1/orgs/{slug}/invitations", get(orgs::list_invitations).post(orgs::invite))
        .route("/api/v1/orgs/{slug}/invitations/{id}", delete(orgs::revoke_invitation))
        .route("/api/v1/orgs/{slug}/teams", get(orgs::list_teams).post(orgs::create_team))
        .route("/api/v1/orgs/{slug}/teams/{id}", delete(orgs::delete_team))
        .route("/api/v1/orgs/{slug}/teams/{id}/members", get(orgs::list_team_members).post(orgs::add_team_member))
        .route("/api/v1/orgs/{slug}/teams/{id}/members/{user_id}", delete(orgs::remove_team_member))
        .route("/api/v1/orgs/{slug}/roles", get(orgs::list_roles).post(orgs::create_role))
        .route("/api/v1/orgs/{slug}/roles/{id}", delete(orgs::delete_role))
        .route("/api/v1/orgs/{slug}/audit", get(orgs::audit_log))
        .route("/api/v1/orgs/{slug}/api-keys", get(orgs::list_api_keys).post(orgs::create_api_key))
        .route("/api/v1/orgs/{slug}/api-keys/{id}", delete(orgs::revoke_api_key))
        .route("/api/v1/orgs/{slug}/api-keys/{id}/rotate", post(orgs::rotate_api_key))
        .route("/api/v1/orgs/{slug}/service-clients", post(orgs::register_service_client))
        .route("/api/v1/orgs/{slug}/billing", get(orgs::billing))
        .route("/api/v1/orgs/{slug}/runs", get(orgs::list_runs).post(orgs::create_run))
        .route("/api/v1/orgs/{slug}/analytics/runs", get(orgs::run_analytics))
        .route("/api/v1/orgs/{slug}/runs/{id}", get(orgs::get_run).delete(orgs::delete_run));
    if admin_enabled {
        r = r
            .route("/api/v1/admin/overview", get(admin::overview))
            .route("/api/v1/admin/users", get(admin::list_users))
            .route("/api/v1/admin/users/{id}", get(admin::get_user).patch(admin::update_user))
            .route("/api/v1/admin/users/{id}/revoke-sessions", post(admin::revoke_user_sessions))
            .route("/api/v1/admin/organizations", get(admin::list_orgs))
            .route("/api/v1/admin/roles", get(admin::roles_model))
            .route("/api/v1/admin/audit", get(admin::audit_log))
            .route("/api/v1/admin/jobs", get(admin::list_jobs))
            .route("/api/v1/admin/jobs/{id}/retry", post(admin::retry_job))
            .route("/api/v1/admin/providers", get(admin::providers))
            .route("/api/v1/admin/system", get(admin::system));
    }
    r
}

/// Authenticated long-lived streams.
pub fn stream_routes() -> Router<AppState> {
    Router::new().route("/api/v1/events", get(realtime::user_sse)).route("/api/v1/ws", get(realtime::user_ws))
}
