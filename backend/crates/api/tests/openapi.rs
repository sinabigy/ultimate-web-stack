#![allow(clippy::unwrap_used)]
//! The OpenAPI document (`app_api::openapi`) against the code it describes:
//! - the committed copy `docs/api/openapi.json` is current (reviewable diffs for integrators);
//! - every documented operation is a real route, and every `/api/v1` route is either documented
//!   or deliberately browser-only;
//! - against real PostgreSQL, every documented operation refuses a request without credentials
//!   (401, not 404/405), and the responses an API key receives match the documented schemas.
//!
//! After an intended API change: `UPDATE_OPENAPI=1 cargo test -p app-api --test openapi`.

mod support;

use std::collections::{BTreeSet, HashMap};

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use support::TestApp;

/// Routes that serve the browser (cookie session + CSRF) or manage credentials, and are therefore
/// not part of the integration surface. A new route must be added here or to the document.
const BROWSER_ONLY: &[&str] = &[
    "GET /api/v1/session",
    "GET /api/v1/dashboard",
    "GET /api/v1/account/profile",
    "PATCH /api/v1/account/profile",
    "PUT /api/v1/account/preferences",
    "GET /api/v1/account/security",
    "DELETE /api/v1/account/security/methods/{kind}/{id}",
    "POST /api/v1/account/security/verify-email",
    "GET /api/v1/account/sessions",
    "DELETE /api/v1/account/sessions/{id}",
    "POST /api/v1/account/sessions/revoke-all",
    "GET /api/v1/account/activity",
    "POST /api/v1/account/delete",
    "GET /api/v1/notifications",
    "POST /api/v1/notifications/read-all",
    "POST /api/v1/notifications/{id}/read",
    "GET /api/v1/invitations/{token}",
    "POST /api/v1/invitations/{token}/accept",
    "GET /api/v1/orgs",
    "POST /api/v1/orgs",
    "GET /api/v1/orgs/{slug}",
    "PATCH /api/v1/orgs/{slug}",
    "DELETE /api/v1/orgs/{slug}",
    "GET /api/v1/orgs/{slug}/overview",
    "POST /api/v1/orgs/{slug}/leave",
    "PATCH /api/v1/orgs/{slug}/members/{user_id}",
    "DELETE /api/v1/orgs/{slug}/members/{user_id}",
    "GET /api/v1/orgs/{slug}/invitations",
    "POST /api/v1/orgs/{slug}/invitations",
    "DELETE /api/v1/orgs/{slug}/invitations/{id}",
    "POST /api/v1/orgs/{slug}/teams",
    "DELETE /api/v1/orgs/{slug}/teams/{id}",
    "GET /api/v1/orgs/{slug}/teams/{id}/members",
    "POST /api/v1/orgs/{slug}/teams/{id}/members",
    "DELETE /api/v1/orgs/{slug}/teams/{id}/members/{user_id}",
    "POST /api/v1/orgs/{slug}/roles",
    "DELETE /api/v1/orgs/{slug}/roles/{id}",
    "GET /api/v1/orgs/{slug}/api-keys",
    "POST /api/v1/orgs/{slug}/api-keys",
    "DELETE /api/v1/orgs/{slug}/api-keys/{id}",
    "POST /api/v1/orgs/{slug}/api-keys/{id}/rotate",
    "POST /api/v1/orgs/{slug}/service-clients",
    "GET /api/v1/orgs/{slug}/billing",
    "GET /api/v1/openapi.json",
    "GET /api/v1/events",
    "GET /api/v1/ws",
];
/// The system administration console (system admins with MFA, in the browser).
const BROWSER_ONLY_PREFIXES: &[&str] = &["/api/v1/admin/"];

fn document() -> Value {
    serde_json::from_str(app_api::openapi::document()).unwrap()
}

/// `METHOD /path` for every documented operation.
fn documented(doc: &Value) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for method in item.as_object().unwrap().keys() {
            out.insert(format!("{} {path}", method.to_uppercase()));
        }
    }
    out
}

/// `METHOD /path` for every route registered in `src/routes/mod.rs` (the single place routes are
/// declared), from `.route("/path", get(..).post(..))`.
fn registered() -> BTreeSet<String> {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/routes/mod.rs")).unwrap();
    let mut out = BTreeSet::new();
    for chunk in src.split(".route(\"").skip(1) {
        let (path, rest) = chunk.split_once('"').unwrap();
        let call = &rest[..rest.find(")\n").unwrap_or(rest.len())];
        for method in ["get", "post", "put", "patch", "delete"] {
            if call.contains(&format!("{method}(")) {
                out.insert(format!("{} {path}", method.to_uppercase()));
            }
        }
    }
    out
}

#[test]
fn committed_document_is_current() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../docs/api/openapi.json");
    let generated = format!("{}\n", app_api::openapi::document());
    if std::env::var_os("UPDATE_OPENAPI").is_some() {
        std::fs::write(path, &generated).unwrap();
    }
    let committed = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        committed == generated,
        "docs/api/openapi.json is stale: run `UPDATE_OPENAPI=1 cargo test -p app-api --test openapi` and review the diff"
    );
}

#[test]
fn documented_operations_are_routes_and_every_route_is_classified() {
    let doc = documented(&document());
    let routes = registered();
    assert!(routes.len() > 40, "route parser found {} routes", routes.len());
    let phantom: Vec<_> = doc.difference(&routes).collect();
    assert!(phantom.is_empty(), "documented but not routed: {phantom:?}");
    let browser: BTreeSet<String> = BROWSER_ONLY.iter().map(|s| (*s).to_string()).collect();
    let stale: Vec<_> = browser.difference(&routes).collect();
    assert!(stale.is_empty(), "BROWSER_ONLY lists routes that no longer exist: {stale:?}");
    let admin =
        |r: &str| BROWSER_ONLY_PREFIXES.iter().any(|p| r.split_once(' ').is_some_and(|(_, path)| path.starts_with(p)));
    let unclassified: Vec<_> = routes
        .iter()
        .filter(|r| r.contains(" /api/v1/") && !doc.contains(*r) && !browser.contains(*r) && !admin(r))
        .collect();
    assert!(unclassified.is_empty(), "new routes must be documented or listed as BROWSER_ONLY: {unclassified:?}");
    assert_eq!(doc.len(), 10);
}

fn resolve<'a>(doc: &'a Value, schema: &'a Value) -> &'a Value {
    match schema["$ref"].as_str() {
        Some(r) => &doc["components"]["schemas"][r.trim_start_matches("#/components/schemas/")],
        None => schema,
    }
}

/// A live JSON value conforms to a documented schema: object keys are exactly the documented
/// properties (required ones present), arrays are checked through their first item, types match.
fn conforms(doc: &Value, schema: &Value, value: &Value, at: &str) {
    let schema = resolve(doc, schema);
    if let Some(variants) = schema["oneOf"].as_array().or_else(|| schema["anyOf"].as_array()) {
        let fits = variants.iter().any(|v| {
            let v = resolve(doc, v);
            v["type"] == "null" && value.is_null() || !value.is_null() && v["type"] != "null"
        });
        assert!(fits, "{at}: {value} matches none of {variants:?}");
        if let Some(v) = variants.iter().map(|v| resolve(doc, v)).find(|v| v["type"] != "null")
            && !value.is_null()
        {
            conforms(doc, v, value, at);
        }
        return;
    }
    let types: Vec<&str> = match &schema["type"] {
        Value::String(t) => vec![t.as_str()],
        Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    if value.is_null() {
        assert!(types.contains(&"null") || types.is_empty(), "{at}: null not allowed by {schema}");
        return;
    }
    match value {
        Value::Object(obj) if types.contains(&"object") => {
            let props = schema["properties"].as_object().cloned().unwrap_or_default();
            if !props.is_empty() {
                let keys: BTreeSet<&String> = obj.keys().collect();
                let documented: BTreeSet<&String> = props.keys().collect();
                assert_eq!(keys, documented, "{at}: live keys vs documented properties");
            }
            for req in schema["required"].as_array().into_iter().flatten() {
                assert!(obj.contains_key(req.as_str().unwrap()), "{at}: required {req} missing");
            }
            for (k, v) in obj {
                if let Some(s) = props.get(k) {
                    conforms(doc, s, v, &format!("{at}.{k}"));
                }
            }
        }
        Value::Array(items) if types.contains(&"array") => {
            if let Some(first) = items.first() {
                conforms(doc, &schema["items"], first, &format!("{at}[0]"));
            }
        }
        Value::String(_) => assert!(types.contains(&"string"), "{at}: string vs {types:?}"),
        Value::Bool(_) => assert!(types.contains(&"boolean"), "{at}: bool vs {types:?}"),
        Value::Number(_) => {
            assert!(types.iter().any(|t| *t == "integer" || *t == "number"), "{at}: number vs {types:?}")
        }
        _ => assert!(types.is_empty() || types.contains(&"object"), "{at}: {value} vs {types:?}"),
    }
}

#[sqlx::test(migrator = "app_db::MIGRATOR")]
async fn documented_operations_refuse_anonymous_and_match_live_responses(pool: PgPool) {
    let doc = document();
    let app = TestApp::new(pool).await;
    let owner = app.login("integrator@openapi.example").await;
    let slug = app.create_org(&owner, "Integrations", "integrations").await.to_string();
    let scopes = json!([
        "runs:read",
        "runs:create",
        "runs:manage_own",
        "audit:read",
        "members:read",
        "teams:read",
        "roles:read"
    ]);
    let key =
        app.post(&owner, &format!("/api/v1/orgs/{slug}/api-keys"), json!({"name": "openapi", "scopes": scopes})).await;
    assert_eq!(key.status, StatusCode::CREATED, "{}", key.body);
    let key = key.body["key"].as_str().unwrap().to_string();

    let created = app
        .bearer(
            &key,
            Method::POST,
            &format!("/api/v1/orgs/{slug}/runs"),
            Some(json!({"label": "openapi", "provider": "simulated", "requested": 1})),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let run_id = created.body["id"].as_str().unwrap().to_string();
    let params: HashMap<&str, &str> = HashMap::from([("{slug}", slug.as_str()), ("{id}", run_id.as_str())]);

    for (path, item) in doc["paths"].as_object().unwrap() {
        let concrete = params.iter().fold(path.clone(), |p, (k, v)| p.replace(k, v));
        for (method, op) in item.as_object().unwrap() {
            let m = Method::from_bytes(method.to_uppercase().as_bytes()).unwrap();
            // `security: [{}]` marks a public operation; otherwise the document-wide bearer applies.
            let public = op["security"]
                .as_array()
                .is_some_and(|a| a.iter().any(|r| r.as_object().is_some_and(|o| o.is_empty())));
            if !public {
                let anon = app.call(None, m.clone(), &concrete, None).await;
                assert_eq!(anon.status, StatusCode::UNAUTHORIZED, "{method} {path} without credentials");
            }
            if m != Method::GET {
                continue;
            }
            let res = app.bearer(&key, m, &concrete, None).await;
            if path.ends_with("/analytics/runs") && res.status == StatusCode::NOT_FOUND {
                continue; // analytics module not compiled in or not enabled in this test
            }
            assert_eq!(res.status, StatusCode::OK, "GET {path}: {}", res.body);
            let schema = &op["responses"]["200"]["content"]["application/json"]["schema"];
            conforms(&doc, schema, &res.body, &format!("GET {path}"));
        }
    }
    // The created run conforms too (POST response).
    let schema =
        &doc["paths"]["/api/v1/orgs/{slug}/runs"]["post"]["responses"]["201"]["content"]["application/json"]["schema"];
    conforms(&doc, schema, &created.body, "POST runs");
}
