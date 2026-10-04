//! A deliberately small OpenID Connect provider for automated tests and offline development.
//!
//! Implements: discovery, JWKS (ES256), authorization code flow with PKCE S256 (mandatory),
//! nonce, `prompt`, `login_hint`, a login form, token endpoint (authorization_code and
//! client_credentials), userinfo, RP-initiated logout. One-shot **fault injection**
//! (`POST /__mock/fault`) makes the next issued token expired, wrong-audience, wrong-issuer,
//! badly signed, wrong-nonce or email-less, so relying-party rejection paths are testable.
//!
//! It is not a security product: there are no passwords. It must never be deployed; it
//! renders a warning banner on every page and is excluded from release images.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Form, Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use p256::pkcs8::{EncodePrivateKey, LineEnding};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct MockClient {
    pub id: String,
    /// None = public client (PKCE only).
    pub secret: Option<String>,
    pub redirect_uris: Vec<String>,
    pub post_logout_redirect_uris: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MockConfig {
    pub clients: Vec<MockClient>,
    /// Service accounts for client_credentials: (client_id, secret, scopes).
    pub service_accounts: Vec<(String, String, Vec<String>)>,
    pub service_audience: String,
    /// Claim used for project roles (ZITADEL object form).
    pub roles_claim: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    Expired,
    WrongAudience,
    WrongIssuer,
    BadSignature,
    WrongNonce,
    NoEmail,
    /// ID token `aud` = [client_id, "extra-audience"] (as ZITADEL does with its project id).
    ExtraAudience,
}

struct Key {
    kid: String,
    encoding: EncodingKey,
    jwk: Value,
}

fn new_key(kid: &str) -> anyhow::Result<Key> {
    let secret = loop {
        let mut bytes = [0u8; 32];
        rand::fill(&mut bytes);
        if let Ok(k) = p256::SecretKey::from_slice(&bytes) {
            break k;
        }
    };
    let pem = secret.to_pkcs8_pem(LineEnding::LF).map_err(|e| anyhow::anyhow!("{e}"))?;
    let encoding = EncodingKey::from_ec_pem(pem.as_bytes())?;
    let mut jwk: Value = serde_json::from_str(&secret.public_key().to_jwk_string())?;
    jwk["kid"] = json!(kid);
    jwk["alg"] = json!("ES256");
    jwk["use"] = json!("sig");
    Ok(Key { kid: kid.to_string(), encoding, jwk })
}

struct CodeGrant {
    client_id: String,
    redirect_uri: String,
    nonce: Option<String>,
    challenge: String,
    user: MockUser,
    issued: SystemTime,
}

#[derive(Clone, Debug)]
struct MockUser {
    email: String,
    name: String,
    amr: Vec<String>,
    roles: Vec<String>,
    verified: bool,
}

impl MockUser {
    fn sub(&self) -> String {
        let h = Sha256::digest(self.email.to_ascii_lowercase().as_bytes());
        format!("mock-{}", hex16(&h))
    }
}

fn hex16(b: &[u8]) -> String {
    b.iter().take(8).map(|x| format!("{x:02x}")).collect()
}

struct Inner {
    issuer: String,
    cfg: MockConfig,
    key: Key,
    rogue_key: Key,
    codes: Mutex<HashMap<String, CodeGrant>>,
    access_tokens: Mutex<HashMap<String, MockUser>>,
    fault: Mutex<Option<Fault>>,
}

type St = Arc<Inner>;

pub struct RunningMock {
    pub issuer: String,
    pub addr: SocketAddr,
    handle: tokio::task::JoinHandle<()>,
}

impl RunningMock {
    pub fn abort(&self) {
        self.handle.abort();
    }
}

impl Drop for RunningMock {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Start on `bind` (use port 0 in tests). The issuer is `http://<addr>`.
pub async fn start(bind: SocketAddr, cfg: MockConfig) -> anyhow::Result<RunningMock> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    let issuer = format!("http://{addr}");
    let state = Arc::new(Inner {
        issuer: issuer.clone(),
        cfg,
        key: new_key("mock-key-1")?,
        rogue_key: new_key("mock-key-1")?, // same kid, different key → signature failure
        codes: Mutex::new(HashMap::new()),
        access_tokens: Mutex::new(HashMap::new()),
        fault: Mutex::new(None),
    });
    let app = router(state);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(RunningMock { issuer, addr, handle })
}

fn router(state: St) -> Router {
    Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/jwks", get(jwks))
        .route("/authorize", get(authorize_page))
        .route("/authorize/complete", post(authorize_complete))
        .route("/token", post(token))
        .route("/userinfo", get(userinfo))
        .route("/end_session", get(end_session))
        .route("/__mock/fault", post(set_fault))
        .with_state(state)
}

async fn discovery(State(s): State<St>) -> Json<Value> {
    let i = &s.issuer;
    Json(json!({
        "issuer": i,
        "authorization_endpoint": format!("{i}/authorize"),
        "token_endpoint": format!("{i}/token"),
        "userinfo_endpoint": format!("{i}/userinfo"),
        "jwks_uri": format!("{i}/jwks"),
        "end_session_endpoint": format!("{i}/end_session"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["ES256"],
        "scopes_supported": ["openid", "profile", "email"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post", "none"],
        "code_challenge_methods_supported": ["S256"],
        "grant_types_supported": ["authorization_code", "client_credentials"],
        "claims_supported": ["sub", "iss", "aud", "exp", "iat", "auth_time", "nonce", "amr", "email", "email_verified", "name"]
    }))
}

async fn jwks(State(s): State<St>) -> Json<Value> {
    Json(json!({ "keys": [s.key.jwk.clone()] }))
}

#[derive(Deserialize)]
struct AuthorizeQuery {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    scope: Option<String>,
    state: Option<String>,
    nonce: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    login_hint: Option<String>,
    prompt: Option<String>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

fn bad(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_request", "error_description": msg}))).into_response()
}

fn validate_authz(s: &Inner, q: &AuthorizeQuery) -> Result<(), Box<Response>> {
    let err = |m: &str| Box::new(bad(m));
    let client = s.cfg.clients.iter().find(|c| c.id == q.client_id).ok_or_else(|| err("unknown client"))?;
    if !client.redirect_uris.contains(&q.redirect_uri) {
        return Err(err("redirect_uri not registered"));
    }
    if q.response_type != "code" {
        return Err(err("unsupported response_type"));
    }
    if q.code_challenge.is_none() || q.code_challenge_method.as_deref() != Some("S256") {
        return Err(err("PKCE S256 required"));
    }
    if !q.scope.as_deref().unwrap_or("").split_whitespace().any(|x| x == "openid") {
        return Err(err("openid scope required"));
    }
    Ok(())
}

async fn authorize_page(State(s): State<St>, Query(q): Query<AuthorizeQuery>) -> Response {
    if let Err(r) = validate_authz(&s, &q) {
        return *r;
    }
    let hidden = |n: &str, v: &Option<String>| {
        v.as_ref().map(|v| format!(r#"<input type="hidden" name="{n}" value="{}">"#, esc(v))).unwrap_or_default()
    };
    let title = match q.prompt.as_deref() {
        Some("create") => "Create account (mock IdP)",
        Some("login") => "Re-authenticate (mock IdP)",
        _ => "Sign in (mock IdP)",
    };
    let email = esc(q.login_hint.as_deref().unwrap_or(""));
    let fields = [
        hidden("client_id", &Some(q.client_id.clone())),
        hidden("redirect_uri", &Some(q.redirect_uri.clone())),
        hidden("scope", &q.scope),
        hidden("state", &q.state),
        hidden("nonce", &q.nonce),
        hidden("code_challenge", &q.code_challenge),
        hidden("code_challenge_method", &q.code_challenge_method),
        hidden("response_type", &Some(q.response_type.clone())),
    ]
    .concat();
    Html(format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>{title}</title></head>
<body style="font-family:system-ui;max-width:28rem;margin:3rem auto">
<p role="alert" style="background:#fde68a;padding:.5rem">Mock identity provider for development and tests. Never deploy.</p>
<h1>{title}</h1>
<form method="post" action="/authorize/complete">{fields}
<p><label>Email <input name="email" type="email" required value="{email}" autocomplete="username"></label></p>
<p><label>Name <input name="name" value=""></label></p>
<p><label>Method <select name="amr"><option value="pwd">Password</option><option value="mfa">Password + TOTP (MFA)</option><option value="passkey">Passkey</option></select></label></p>
<p><label>IdP roles (comma separated) <input name="roles" value=""></label></p>
<p><button type="submit">Continue</button></p>
</form></body></html>"#
    ))
    .into_response()
}

#[derive(Deserialize)]
struct CompleteForm {
    client_id: String,
    redirect_uri: String,
    response_type: String,
    scope: Option<String>,
    state: Option<String>,
    nonce: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    email: String,
    name: Option<String>,
    amr: Option<String>,
    roles: Option<String>,
    email_verified: Option<bool>,
}

async fn authorize_complete(State(s): State<St>, Form(f): Form<CompleteForm>) -> Response {
    let q = AuthorizeQuery {
        response_type: f.response_type.clone(),
        client_id: f.client_id.clone(),
        redirect_uri: f.redirect_uri.clone(),
        scope: f.scope.clone(),
        state: f.state.clone(),
        nonce: f.nonce.clone(),
        code_challenge: f.code_challenge.clone(),
        code_challenge_method: f.code_challenge_method.clone(),
        login_hint: None,
        prompt: None,
    };
    if let Err(r) = validate_authz(&s, &q) {
        return *r;
    }
    let amr = match f.amr.as_deref() {
        Some("mfa") => vec!["pwd".to_string(), "otp".to_string(), "mfa".to_string()],
        Some("passkey") => vec!["hwk".to_string(), "user".to_string()],
        _ => vec!["pwd".to_string()],
    };
    let email = f.email.trim().to_string();
    let user = MockUser {
        name: f
            .name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| email.split('@').next().unwrap_or("user").to_string()),
        email,
        amr,
        roles: f.roles.unwrap_or_default().split(',').map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect(),
        verified: f.email_verified.unwrap_or(true),
    };
    let code = token_string();
    s.codes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(
        code.clone(),
        CodeGrant {
            client_id: f.client_id,
            redirect_uri: f.redirect_uri.clone(),
            nonce: f.nonce,
            challenge: f.code_challenge.unwrap_or_default(),
            user,
            issued: SystemTime::now(),
        },
    );
    let Ok(mut url) = url::Url::parse(&f.redirect_uri) else { return bad("bad redirect_uri") };
    url.query_pairs_mut().append_pair("code", &code);
    if let Some(st) = &f.state {
        url.query_pairs_mut().append_pair("state", st);
    }
    Redirect::to(url.as_str()).into_response()
}

fn token_string() -> String {
    let mut b = [0u8; 32];
    rand::fill(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[derive(Deserialize)]
struct TokenForm {
    grant_type: String,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    scope: Option<String>,
}

fn basic_auth(h: &HeaderMap) -> Option<(String, String)> {
    let v = h.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Basic ")?;
    let raw = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(v).ok()?).ok()?;
    let (id, secret) = raw.split_once(':')?;
    let dec = |s: &str| url::form_urlencoded::parse(format!("x={s}").as_bytes()).next().map(|(_, v)| v.into_owned());
    Some((dec(id)?, dec(secret)?))
}

fn oauth_err(code: &str, desc: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": code, "error_description": desc}))).into_response()
}

async fn token(State(s): State<St>, headers: HeaderMap, Form(f): Form<TokenForm>) -> Response {
    let (client_id, client_secret) = match basic_auth(&headers) {
        Some((i, sec)) => (Some(i), Some(sec)),
        None => (f.client_id.clone(), f.client_secret.clone()),
    };
    let Some(client_id) = client_id else { return oauth_err("invalid_client", "missing client") };
    match f.grant_type.as_str() {
        "authorization_code" => {
            let Some(client) = s.cfg.clients.iter().find(|c| c.id == client_id) else {
                return oauth_err("invalid_client", "unknown client");
            };
            if client.secret.is_some() && client.secret != client_secret {
                return oauth_err("invalid_client", "bad secret");
            }
            let grant = s
                .codes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(f.code.as_deref().unwrap_or(""));
            let Some(g) = grant else { return oauth_err("invalid_grant", "unknown or used code") };
            if g.client_id != client_id || Some(&g.redirect_uri) != f.redirect_uri.as_ref() {
                return oauth_err("invalid_grant", "client/redirect mismatch");
            }
            if g.issued.elapsed().unwrap_or(Duration::MAX) > Duration::from_secs(60) {
                return oauth_err("invalid_grant", "code expired");
            }
            let verifier = f.code_verifier.unwrap_or_default();
            if URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) != g.challenge {
                return oauth_err("invalid_grant", "PKCE verification failed");
            }
            let fault = s.fault.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
            let id_token = id_token(&s, &client_id, &g, fault);
            let access = token_string();
            s.access_tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(access.clone(), g.user.clone());
            Json(json!({"access_token": access, "token_type": "Bearer", "expires_in": 300, "id_token": id_token}))
                .into_response()
        }
        "client_credentials" => {
            let Some((_, secret, scopes)) = s.cfg.service_accounts.iter().find(|(id, _, _)| *id == client_id) else {
                return oauth_err("invalid_client", "unknown service account");
            };
            if Some(secret) != client_secret.as_ref() {
                return oauth_err("invalid_client", "bad secret");
            }
            let requested: Vec<&str> = f.scope.as_deref().unwrap_or("").split_whitespace().collect();
            // Like real IdPs: grant the requested scopes the account may have, plus `openid` if asked.
            let openid = "openid".to_string();
            let mut granted: Vec<&String> =
                scopes.iter().filter(|sc| requested.is_empty() || requested.contains(&sc.as_str())).collect();
            if requested.contains(&"openid") {
                granted.push(&openid);
            }
            let fault = s.fault.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
            let claims = json!({
                "iss": if fault == Some(Fault::WrongIssuer) { "https://evil.example".to_string() } else { s.issuer.clone() },
                "sub": client_id, "client_id": client_id,
                "aud": if fault == Some(Fault::WrongAudience) { "someone-else".to_string() } else { s.cfg.service_audience.clone() },
                "iat": now(), "exp": if fault == Some(Fault::Expired) { now() - 3600 } else { now() + 300 },
                "scope": granted.iter().map(|x| x.as_str()).collect::<Vec<_>>().join(" "),
            });
            let key = if fault == Some(Fault::BadSignature) { &s.rogue_key } else { &s.key };
            let mut h = Header::new(Algorithm::ES256);
            h.kid = Some(key.kid.clone());
            h.typ = Some("at+jwt".into());
            match jsonwebtoken::encode(&h, &claims, &key.encoding) {
                Ok(t) => Json(json!({"access_token": t, "token_type": "Bearer", "expires_in": 300})).into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            }
        }
        _ => oauth_err("unsupported_grant_type", "unsupported"),
    }
}

fn id_token(s: &Inner, client_id: &str, g: &CodeGrant, fault: Option<Fault>) -> String {
    let u = &g.user;
    let mut claims = json!({
        "iss": if fault == Some(Fault::WrongIssuer) { "https://evil.example".to_string() } else { s.issuer.clone() },
        "sub": u.sub(),
        "aud": match fault {
            Some(Fault::WrongAudience) => json!("someone-else"),
            Some(Fault::ExtraAudience) => json!([client_id, "extra-audience"]),
            _ => json!(client_id),
        },
        "azp": client_id,
        "iat": now(),
        "exp": if fault == Some(Fault::Expired) { now() - 3600 } else { now() + 300 },
        "auth_time": now(),
        "amr": u.amr,
        "name": u.name,
    });
    if fault != Some(Fault::NoEmail) {
        claims["email"] = json!(u.email);
        claims["email_verified"] = json!(u.verified);
    }
    match (&g.nonce, fault) {
        (_, Some(Fault::WrongNonce)) => claims["nonce"] = json!("not-the-nonce"),
        (Some(n), _) => claims["nonce"] = json!(n),
        _ => {}
    }
    if !u.roles.is_empty() {
        let obj: serde_json::Map<String, Value> =
            u.roles.iter().map(|r| (r.clone(), json!({"mock-org": "mock"}))).collect();
        claims[&s.cfg.roles_claim] = Value::Object(obj);
    }
    let key = if fault == Some(Fault::BadSignature) { &s.rogue_key } else { &s.key };
    let mut h = Header::new(Algorithm::ES256);
    h.kid = Some(key.kid.clone());
    jsonwebtoken::encode(&h, &claims, &key.encoding).unwrap_or_default()
}

async fn userinfo(State(s): State<St>, headers: HeaderMap) -> Response {
    let token =
        headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    let user = token.and_then(|t| s.access_tokens.lock().ok()?.get(t).cloned());
    match user {
        Some(u) => Json(json!({"sub": u.sub(), "email": u.email, "email_verified": u.verified, "name": u.name}))
            .into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

#[derive(Deserialize)]
struct EndSessionQuery {
    post_logout_redirect_uri: Option<String>,
    client_id: Option<String>,
    state: Option<String>,
}

async fn end_session(State(s): State<St>, Query(q): Query<EndSessionQuery>) -> Response {
    let allowed = q.post_logout_redirect_uri.as_ref().filter(|u| {
        s.cfg
            .clients
            .iter()
            .any(|c| q.client_id.as_deref().is_none_or(|id| id == c.id) && c.post_logout_redirect_uris.contains(u))
    });
    match allowed {
        Some(u) => {
            let mut url = match url::Url::parse(u) {
                Ok(u) => u,
                Err(_) => return bad("bad post_logout_redirect_uri"),
            };
            if let Some(st) = &q.state {
                url.query_pairs_mut().append_pair("state", st);
            }
            Redirect::to(url.as_str()).into_response()
        }
        None => Html("<p>Signed out of the mock IdP.</p>").into_response(),
    }
}

#[derive(Deserialize)]
struct FaultBody {
    fault: Option<Fault>,
}

async fn set_fault(State(s): State<St>, Json(b): Json<FaultBody>) -> StatusCode {
    *s.fault.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = b.fault;
    StatusCode::NO_CONTENT
}
