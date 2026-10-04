#![allow(clippy::unwrap_used)]
//! Authentication security tests (BFF + OIDC). Each runs against a fresh database and a real
//! (mock) OIDC provider.

mod support;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use serde_json::json;
use sqlx::PgPool;
use support::*;
use tower::ServiceExt;

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn unauthenticated_access_is_rejected(pool: PgPool) {
    let app = TestApp::new(pool).await;
    for path in ["/api/v1/dashboard", "/api/v1/account/profile", "/api/v1/orgs", "/api/v1/admin/users"] {
        let r = app.call(None, Method::GET, path, None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{path}");
    }
    let s = app.call(None, Method::GET, "/api/v1/session", None).await;
    assert_eq!(s.body["authenticated"], false);
    assert!(s.body["login"]["passkey"].is_boolean());
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn full_login_creates_user_personal_org_and_session(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let b = app.login("ada@example.com").await;
    let s = app.get(&b, "/api/v1/session").await;
    assert_eq!(s.body["user"]["email"], "ada@example.com");
    assert_eq!(s.body["session"]["mfa"], false);
    assert_eq!(s.body["organizations"].as_array().unwrap().len(), 1, "personal organisation");
    assert_eq!(app.audit_count("user.created").await, 1);
    assert_eq!(app.audit_count("user.login").await, 1);
    // Second login: same user, no new personal org.
    let b2 = app.login("ada@example.com").await;
    assert_eq!(b.user_id, b2.user_id);
    assert_eq!(app.audit_count("user.created").await, 1);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn session_cookie_attributes(pool: PgPool) {
    let app = TestApp::with_config(pool, |c| c.auth.cookie_secure = true).await;
    let (authorize, flow) = app.begin("/auth/login").await;
    let cb = app.idp_complete(&authorize, "c@example.com", "pwd", "").await;
    let r = app
        .raw(
            Request::get(&cb)
                .header(header::COOKIE, format!("__Secure-app_session_flow={flow}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    let cookie = r
        .headers
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .find(|c| c.starts_with("__Host-app_session="))
        .expect("host-prefixed session cookie");
    for attr in ["HttpOnly", "SameSite=Lax", "Path=/", "Secure"] {
        assert!(cookie.contains(attr), "{cookie} missing {attr}");
    }
    assert!(!cookie.contains("Domain="));
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn browser_never_receives_tokens(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let b = app.login("tok@example.com").await;
    let s = app.get(&b, "/api/v1/session").await;
    let text = s.body.to_string();
    assert!(!text.contains("id_token") && !text.contains("access_token") && !text.contains("refresh_token"));
    assert!(!text.contains("eyJ"), "no JWT material in session payload");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn forged_expired_and_revoked_sessions_are_rejected(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let b = app.login("s@example.com").await;
    // forged cookie
    let forged = Browser { session: "A".repeat(43), ..b.clone() };
    assert_eq!(app.get(&forged, "/api/v1/dashboard").await.status, StatusCode::UNAUTHORIZED);
    let garbage = Browser { session: "not-a-token".into(), ..b.clone() };
    let r = app.get(&garbage, "/api/v1/dashboard").await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(set_cookie(&r.headers, "app_session").as_deref(), Some(""), "dead cookie is cleared");
    // expired (absolute)
    sqlx::query("UPDATE sessions SET expires_at = now() - interval '1 second' WHERE user_id = $1")
        .bind(b.user_id)
        .execute(&app.pool)
        .await
        .unwrap();
    assert_eq!(app.get(&b, "/api/v1/dashboard").await.status, StatusCode::UNAUTHORIZED);
    // idle-expired
    let b2 = app.login("s@example.com").await;
    sqlx::query("UPDATE sessions SET idle_expires_at = now() - interval '1 second' WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > now()")
        .bind(b2.user_id)
        .execute(&app.pool)
        .await
        .unwrap();
    assert_eq!(app.get(&b2, "/api/v1/dashboard").await.status, StatusCode::UNAUTHORIZED);
    // revoked
    let b3 = app.login("s@example.com").await;
    assert_eq!(app.get(&b3, "/api/v1/dashboard").await.status, StatusCode::OK);
    let sid = app.get(&b3, "/api/v1/session").await.body["session"]["id"].as_str().unwrap().to_string();
    let b4 = app.login("s@example.com").await;
    assert_eq!(app.delete(&b4, &format!("/api/v1/account/sessions/{sid}")).await.status, StatusCode::NO_CONTENT);
    assert_eq!(app.get(&b3, "/api/v1/dashboard").await.status, StatusCode::UNAUTHORIZED, "revoked session");
    assert!(app.audit_count("security.session_revoked").await >= 1);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn csrf_token_required_for_cookie_authenticated_writes(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let b = app.login("csrf@example.com").await;
    let no_token = app
        .raw(
            Request::patch("/api/v1/account/profile")
                .header(header::COOKIE, format!("app_session={}", b.session))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"display_name": "X"}).to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(no_token.status, StatusCode::FORBIDDEN);
    assert_eq!(no_token.code(), "csrf_failed");
    let wrong = Browser { csrf: "wrong".into(), ..b.clone() };
    assert_eq!(app.patch(&wrong, "/api/v1/account/profile", json!({"display_name": "X"})).await.code(), "csrf_failed");
    // cross-site request is rejected before authentication even with the right token
    let cross = app
        .raw(
            Request::patch("/api/v1/account/profile")
                .header(header::COOKIE, format!("app_session={}", b.session))
                .header("x-csrf-token", &b.csrf)
                .header("sec-fetch-site", "cross-site")
                .header("origin", "https://evil.example")
                .header(header::HOST, "localhost:5190")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"display_name": "X"}).to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(cross.status, StatusCode::FORBIDDEN);
    let ok = app.patch(&b, "/api/v1/account/profile", json!({"display_name": "Grace"})).await;
    assert_eq!(ok.status, StatusCode::OK, "{:?}", ok.body);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn invalid_id_tokens_are_rejected(pool: PgPool) {
    let app = TestApp::new(pool).await;
    for fault in ["expired", "wrong_audience", "wrong_issuer", "bad_signature", "wrong_nonce"] {
        app.fault(fault).await;
        let err = app.try_login("evil@example.com", "pwd", "").await.expect_err(fault);
        assert_eq!(err, "/login?error=login_rejected", "{fault}");
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users").fetch_one(&app.pool).await.unwrap();
    assert_eq!(n, 0, "no user created from rejected tokens");
    assert!(app.audit_count("user.login").await >= 5, "failures audited");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn missing_email_claim_falls_back_to_userinfo(pool: PgPool) {
    let app = TestApp::new(pool).await;
    app.fault("no_email").await;
    let b = app.try_login("userinfo@example.com", "pwd", "").await.unwrap();
    assert_eq!(b.email, "userinfo@example.com");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn login_csrf_state_must_match_browser_and_is_single_use(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let (authorize, flow) = app.begin("/auth/login").await;
    let cb = app.idp_complete(&authorize, "victim@example.com", "pwd", "").await;
    // attacker's browser has no / a different flow cookie
    let r = app.callback(&cb, "attacker-flow").await;
    assert_eq!(r.headers[header::LOCATION], "/login?error=state_mismatch");
    let r = app.raw(Request::get(&cb).body(Body::empty()).unwrap()).await;
    assert_eq!(r.headers[header::LOCATION], "/login?error=state_mismatch");
    // legitimate completion works once ...
    let ok = app.callback(&cb, &flow).await;
    assert!(set_cookie(&ok.headers, "app_session").is_some());
    // ... and replaying it fails
    let replay = app.callback(&cb, &flow).await;
    assert_eq!(replay.headers[header::LOCATION], "/login?error=flow_expired");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn return_to_cannot_redirect_off_site(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let (authorize, flow) = app.begin("/auth/login?return_to=https://evil.example/steal").await;
    let cb = app.idp_complete(&authorize, "r@example.com", "pwd", "").await;
    let r = app.callback(&cb, &flow).await;
    assert_eq!(r.headers[header::LOCATION], "/dashboard");
    let (authorize, flow) = app.begin("/auth/login?return_to=/org/acme").await;
    let cb = app.idp_complete(&authorize, "r@example.com", "pwd", "").await;
    assert_eq!(app.callback(&cb, &flow).await.headers[header::LOCATION], "/org/acme");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn pkce_and_prompt_parameters_sent(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let (u, _) = app.begin("/auth/login").await;
    let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
    assert_eq!(q["code_challenge_method"], "S256");
    assert!(q.contains_key("nonce") && q.contains_key("state"));
    // The verifier is not stored in plaintext: if it were, S256(stored) would equal the challenge.
    let stored: String = sqlx::query_scalar("SELECT pkce_verifier FROM oidc_flows").fetch_one(&app.pool).await.unwrap();
    use base64::Engine as _;
    let s256 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(app_auth::tokens::sha256(stored.as_bytes()));
    assert_ne!(s256, q["code_challenge"], "PKCE verifier must be encrypted at rest");
    let (u, _) = app.begin("/auth/register").await;
    assert!(u.query_pairs().any(|(k, v)| k == "prompt" && v == "create"));
    let (u, _) = app.begin("/auth/reauth").await;
    assert!(u.query_pairs().any(|(k, v)| k == "prompt" && v == "login"));
    assert!(u.query_pairs().any(|(k, v)| k == "max_age" && v == "0"));
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn suspended_user_cannot_log_in_and_sessions_end(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let b = app.login("bad@example.com").await;
    sqlx::query("UPDATE users SET status = 'suspended' WHERE id = $1")
        .bind(b.user_id)
        .execute(&app.pool)
        .await
        .unwrap();
    assert_eq!(app.get(&b, "/api/v1/dashboard").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.try_login("bad@example.com", "pwd", "").await.unwrap_err(), "/login?error=account_inactive");
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn logout_revokes_and_returns_idp_end_session(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let b = app.login("bye@example.com").await;
    let r = app.post(&b, "/auth/logout", json!({})).await;
    assert_eq!(r.status, StatusCode::OK);
    let redirect = r.body["redirect"].as_str().unwrap();
    assert!(redirect.starts_with(&format!("{}/end_session", app.mock.issuer)), "{redirect}");
    assert!(redirect.contains("id_token_hint="), "RP-initiated logout carries the hint");
    assert_eq!(set_cookie(&r.headers, "app_session").as_deref(), Some(""));
    assert_eq!(app.get(&b, "/api/v1/dashboard").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.audit_count("user.logout").await, 1);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn logout_everywhere(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let laptop = app.login("multi@example.com").await;
    let phone = app.login("multi@example.com").await;
    let r = app.post(&laptop, "/api/v1/account/sessions/revoke-all", json!({})).await;
    assert_eq!(r.body["count"], 1, "other devices only");
    assert_eq!(app.get(&phone, "/api/v1/dashboard").await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.get(&laptop, "/api/v1/dashboard").await.status, StatusCode::OK);
    let r = app.post(&laptop, "/api/v1/account/sessions/revoke-all", json!({"include_current": true})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(app.get(&laptop, "/api/v1/dashboard").await.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn session_rotation_issues_new_cookie_and_old_one_lapses(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let b = app.login("rot@example.com").await;
    sqlx::query("UPDATE sessions SET rotated_at = now() - interval '2 hours' WHERE user_id = $1")
        .bind(b.user_id)
        .execute(&app.pool)
        .await
        .unwrap();
    let r = app.get(&b, "/api/v1/dashboard").await;
    assert_eq!(r.status, StatusCode::OK);
    let fresh = set_cookie(&r.headers, "app_session").expect("rotated cookie");
    assert_ne!(fresh, b.session);
    let rotated = Browser { session: fresh, ..b.clone() };
    assert_eq!(app.get(&rotated, "/api/v1/dashboard").await.status, StatusCode::OK);
    assert_eq!(app.get(&b, "/api/v1/dashboard").await.status, StatusCode::OK, "old cookie valid during grace");
    sqlx::query("UPDATE sessions SET previous_valid_until = now() - interval '1 second' WHERE user_id = $1")
        .bind(b.user_id)
        .execute(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        app.get(&b, "/api/v1/dashboard").await.status,
        StatusCode::UNAUTHORIZED,
        "old cookie lapses after grace"
    );
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn new_login_replaces_existing_session(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let first = app.login("fix@example.com").await;
    // Log in again from the same browser (sends the old cookie): old session is revoked.
    let (authorize, flow) = app.begin("/auth/login").await;
    let cb = app.idp_complete(&authorize, "fix@example.com", "pwd", "").await;
    let r = app
        .raw(
            Request::get(&cb)
                .header(header::COOKIE, format!("app_session_flow={flow}; app_session={}", first.session))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    let new = set_cookie(&r.headers, "app_session").unwrap();
    assert_ne!(new, first.session, "fresh session id/token (no fixation)");
    assert_eq!(app.get(&first, "/api/v1/dashboard").await.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn mfa_detected_from_amr(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let b = app.try_login("mfa@example.com", "passkey", "").await.unwrap();
    assert_eq!(app.get(&b, "/api/v1/session").await.body["session"]["mfa"], true);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn authenticated_realtime_stream_accepts_session(pool: PgPool) {
    let app = TestApp::new(pool).await;
    let b = app.login("rt@example.com").await;
    let res = app
        .router
        .clone()
        .oneshot(
            Request::get("/api/v1/events")
                .header(header::COOKIE, format!("app_session={}", b.session))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "text/event-stream");
    let anon = app.router.clone().oneshot(Request::get("/api/v1/events").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn extra_id_token_audiences_must_be_explicitly_trusted(pool: PgPool) {
    let app = TestApp::new(pool.clone()).await;
    app.fault("extra_audience").await;
    assert_eq!(app.try_login("aud@example.com", "pwd", "").await.unwrap_err(), "/login?error=login_rejected");
    let trusted =
        TestApp::with_config(pool, |c| c.auth.id_token_trusted_audiences = vec!["extra-audience".into()]).await;
    trusted.fault("extra_audience").await;
    assert!(trusted.try_login("aud@example.com", "pwd", "").await.is_ok());
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn rate_limits_are_per_principal_once_signed_in(pool: PgPool) {
    let app = TestApp::with_config(pool, |c| {
        c.rate_limit.enabled = true;
        c.rate_limit.per_client_rps = 0.01;
        c.rate_limit.burst = 3;
    })
    .await;
    // Same network origin for both users (no client IP): only the principal can tell them apart.
    let a = app.login("rl-a@example.com").await;
    let b = app.login("rl-b@example.com").await;
    // Exhaust a's bucket (login itself already spent some of it).
    let mut limited = false;
    for _ in 0..4 {
        if app.get(&a, "/api/v1/account/profile").await.status == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
    }
    assert!(limited, "a is limited after its burst");
    assert_eq!(app.get(&b, "/api/v1/account/profile").await.status, StatusCode::OK, "b has its own bucket");
}
