#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
//! Full-stack test harness: real PostgreSQL (fresh database per test via `#[sqlx::test]`),
//! the in-repo mock OIDC provider over real HTTP, and the application router driven in-process
//! exactly as a browser would (redirects, cookies, CSRF header).

use std::{net::SocketAddr, sync::Arc};

use app_api::{AppState, BuildInfo};
use app_config::{AppConfig, IdentityProviderKind};
use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use mock_oidc::{MockClient, MockConfig, RunningMock};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

pub const ORIGIN: &str = "http://localhost:5190";

pub struct TestApp {
    pub router: Router,
    pub state: AppState,
    pub pool: PgPool,
    pub mock: RunningMock,
    http: reqwest::Client,
}

#[derive(Debug, Clone)]
pub struct Browser {
    pub session: String,
    pub csrf: String,
    pub user_id: Uuid,
    pub email: String,
}

pub struct Res {
    pub status: StatusCode,
    pub body: Value,
    pub headers: HeaderMap,
}

impl Res {
    pub fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or("")
    }
}

pub fn base_config(issuer: &str) -> AppConfig {
    let mut c = AppConfig { environment: "test".into(), ..Default::default() };
    c.rate_limit.enabled = false;
    c.auth.provider = IdentityProviderKind::Oidc;
    c.auth.issuer_url = issuer.to_string();
    c.auth.client_id = "app-web".into();
    c.auth.public_origin = ORIGIN.into();
    c.auth.redirect_url = format!("{ORIGIN}/auth/callback");
    c.auth.post_logout_redirect_url = format!("{ORIGIN}/login");
    c.auth.service_audiences = vec!["app-api".into()];
    c.auth.require_mfa_for_system_admin = true;
    c
}

impl TestApp {
    pub async fn new(pool: PgPool) -> Self {
        Self::with_config(pool, |_| {}).await
    }

    pub async fn with_config(pool: PgPool, f: impl FnOnce(&mut AppConfig)) -> Self {
        let mock = mock_oidc::start(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            MockConfig {
                clients: vec![MockClient {
                    id: "app-web".into(),
                    secret: None,
                    redirect_uris: vec![format!("{ORIGIN}/auth/callback")],
                    post_logout_redirect_uris: vec![format!("{ORIGIN}/login")],
                }],
                service_accounts: vec![
                    ("svc-reporting".into(), "svc-secret".into(), vec!["runs:read".into(), "runs:create".into()]),
                    ("svc-unregistered".into(), "svc-secret".into(), vec!["runs:read".into()]),
                ],
                service_audience: "app-api".into(),
                roles_claim: "urn:zitadel:iam:org:project:roles".into(),
            },
        )
        .await
        .unwrap();
        let mut cfg = base_config(&mock.issuer);
        f(&mut cfg);
        cfg.validate().expect("test config valid");
        app_db::orgs::sync_permissions(&pool).await.unwrap();
        let services = app_api::bootstrap::build_services(&cfg, pool.clone()).unwrap();
        let limiter = cfg.rate_limit.enabled.then(|| {
            Arc::new(app_rate_limit::MemoryRateLimiter::new(app_rate_limit::Quota {
                per_second: cfg.rate_limit.per_client_rps,
                burst: cfg.rate_limit.burst,
            }))
        });
        let analytics = if cfg.analytics.enabled {
            Some(app_analytics::start(&cfg.analytics).await.expect("analytics"))
        } else {
            None
        };
        let mut builder = AppState::builder(cfg, BuildInfo { name: "t", version: "0", git_sha: "t", profile: "debug" })
            .services(Arc::new(services));
        if let Some(l) = limiter {
            builder = builder.rate_limiter(l);
        }
        if let Some(a) = analytics {
            builder = builder.analytics(a.sink.clone());
            if let Some(q) = a.query.clone() {
                builder = builder.analytics_query(q);
            }
        }
        let state = builder.build();
        let router = app_api::build_router(state.clone());
        let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        Self { router, state, pool, mock, http }
    }

    pub async fn raw(&self, req: Request<Body>) -> Res {
        let res = self.router.clone().oneshot(req).await.unwrap();
        let (parts, body) = res.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes();
        Res {
            status: parts.status,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            headers: parts.headers,
        }
    }

    pub async fn call(&self, b: Option<&Browser>, method: Method, path: &str, body: Option<Value>) -> Res {
        let mut req = Request::builder().method(method.clone()).uri(path);
        if let Some(b) = b {
            req = req.header(header::COOKIE, format!("app_session={}", b.session));
            if method != Method::GET {
                req = req.header("x-csrf-token", &b.csrf);
            }
        }
        let req = match body {
            Some(v) => req.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        self.raw(req).await
    }

    pub async fn get(&self, b: &Browser, path: &str) -> Res {
        self.call(Some(b), Method::GET, path, None).await
    }
    pub async fn post(&self, b: &Browser, path: &str, body: Value) -> Res {
        self.call(Some(b), Method::POST, path, Some(body)).await
    }
    pub async fn patch(&self, b: &Browser, path: &str, body: Value) -> Res {
        self.call(Some(b), Method::PATCH, path, Some(body)).await
    }
    pub async fn delete(&self, b: &Browser, path: &str) -> Res {
        self.call(Some(b), Method::DELETE, path, None).await
    }

    pub async fn bearer(&self, token: &str, method: Method, path: &str, body: Option<Value>) -> Res {
        let mut req =
            Request::builder().method(method).uri(path).header(header::AUTHORIZATION, format!("Bearer {token}"));
        let req = match body {
            Some(v) => {
                req = req.header(header::CONTENT_TYPE, "application/json");
                req.body(Body::from(v.to_string())).unwrap()
            }
            None => req.body(Body::empty()).unwrap(),
        };
        self.raw(req).await
    }

    pub async fn fault(&self, fault: &str) {
        self.http
            .post(format!("{}/__mock/fault", self.mock.issuer))
            .json(&json!({"fault": fault}))
            .send()
            .await
            .unwrap();
    }

    /// Start a login; returns (authorize URL, flow cookie value).
    pub async fn begin(&self, path: &str) -> (url::Url, String) {
        let res = self.raw(Request::get(path).body(Body::empty()).unwrap()).await;
        assert_eq!(res.status, StatusCode::SEE_OTHER, "begin {path}: {:?}", res.body);
        let loc = res.headers[header::LOCATION].to_str().unwrap();
        let flow = set_cookie(&res.headers, "app_session_flow")
            .or_else(|| set_cookie(&res.headers, "__Secure-app_session_flow"))
            .expect("flow cookie");
        (url::Url::parse(loc).unwrap(), flow)
    }

    /// Act as the user at the IdP: returns the callback URL path+query.
    pub async fn idp_complete(&self, authorize: &url::Url, email: &str, amr: &str, roles: &str) -> String {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in authorize.query_pairs() {
            if k != "prompt" && k != "login_hint" && k != "max_age" {
                form.append_pair(&k, &v);
            }
        }
        form.append_pair("email", email).append_pair("amr", amr).append_pair("roles", roles);
        let r = self
            .http
            .post(format!("{}/authorize/complete", self.mock.issuer))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form.finish())
            .send()
            .await
            .unwrap();
        assert!(r.status().is_redirection(), "mock authorize failed: {}", r.text().await.unwrap_or_default());
        let loc = url::Url::parse(r.headers()["location"].to_str().unwrap()).unwrap();
        format!("{}?{}", loc.path(), loc.query().unwrap_or(""))
    }

    pub async fn callback(&self, path: &str, flow_cookie: &str) -> Res {
        self.raw(
            Request::get(path)
                .header(header::COOKIE, format!("app_session_flow={flow_cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    /// Full browser login. `amr`: "pwd" | "mfa" | "passkey"; `roles`: comma-separated IdP roles.
    pub async fn try_login(&self, email: &str, amr: &str, roles: &str) -> Result<Browser, String> {
        let (authorize, flow) = self.begin("/auth/login?return_to=/dashboard").await;
        let cb = self.idp_complete(&authorize, email, amr, roles).await;
        let res = self.callback(&cb, &flow).await;
        let loc = res.headers.get(header::LOCATION).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
        let Some(session) = set_cookie(&res.headers, "app_session").filter(|s| !s.is_empty()) else {
            return Err(loc);
        };
        assert_eq!(loc, "/dashboard");
        let mut b = Browser { session, csrf: String::new(), user_id: Uuid::nil(), email: email.into() };
        let s = self.get(&b, "/api/v1/session").await;
        assert_eq!(s.body["authenticated"], true, "{:?}", s.body);
        b.csrf = s.body["csrf_token"].as_str().unwrap().to_string();
        b.user_id = s.body["user"]["id"].as_str().unwrap().parse().unwrap();
        Ok(b)
    }

    pub async fn login(&self, email: &str) -> Browser {
        self.try_login(email, "pwd", "").await.expect("login")
    }

    pub async fn personal_slug(&self, b: &Browser) -> String {
        let s = self.get(b, "/api/v1/orgs").await;
        s.body["items"].as_array().unwrap().iter().find(|o| o["personal"] == true).unwrap()["slug"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Create a shared organisation owned by `b`.
    pub async fn create_org(&self, b: &Browser, name: &str, slug: &str) -> String {
        let r = self.post(b, "/api/v1/orgs", json!({"name": name, "slug": slug})).await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        slug.to_string()
    }

    /// Invite `member` into `slug` with built-in role `role` and accept as them.
    pub async fn add_member(&self, owner: &Browser, slug: &str, member: &Browser, role: &str) {
        let role_id = builtin_role_id(role);
        let r = self
            .post(
                owner,
                &format!("/api/v1/orgs/{slug}/invitations"),
                json!({"email": member.email, "role_id": role_id}),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        let token = r.body["link"].as_str().unwrap().rsplit('/').next().unwrap().to_string();
        let a = self.post(member, &format!("/api/v1/invitations/{token}/accept"), json!({})).await;
        assert_eq!(a.status, StatusCode::OK, "{:?}", a.body);
    }

    pub async fn audit_count(&self, action: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = $1")
            .bind(action)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    pub async fn set_system_role(&self, user: Uuid, role: &str) {
        sqlx::query("UPDATE users SET system_role = $2 WHERE id = $1")
            .bind(user)
            .bind(role)
            .execute(&self.pool)
            .await
            .unwrap();
    }

    pub async fn service_token(&self, client: &str, scope: &str) -> String {
        let r = self
            .http
            .post(format!("{}/token", self.mock.issuer))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(format!("grant_type=client_credentials&client_id={client}&client_secret=svc-secret&scope={scope}"))
            .send()
            .await
            .unwrap();
        r.json::<Value>().await.unwrap()["access_token"].as_str().unwrap().to_string()
    }
}

pub fn builtin_role_id(key: &str) -> Uuid {
    app_authz::rbac::builtin_role_id(app_domain::OrgRole::parse(key).unwrap())
}

/// Value of a Set-Cookie for `name` (empty string when the cookie is being cleared).
pub fn set_cookie(h: &HeaderMap, name: &str) -> Option<String> {
    h.get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| c.split(';').next()?.trim().strip_prefix(&format!("{name}=")).map(String::from))
}
