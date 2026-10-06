# Integration API (OpenAPI)

[`openapi.json`](openapi.json) is an OpenAPI 3.1 description of the operations that machine
credentials use. Those credentials are organization **API keys** and registered **service
accounts**. A running server serves the same document at `GET /api/v1/openapi.json`.

- **Generated from code:** the schemas derive from the Rust types the handlers return (`utoipa`, in
  `backend/crates/api/src/openapi.rs`), so the document cannot describe a shape the API does not
  send.
- **Tested against the server:** `backend/crates/api/tests/openapi.rs` checks four things:
  - every documented operation is a real route, and every `/api/v1` route is either documented or
    explicitly browser-only;
  - each documented operation answers 401 without credentials;
  - with a real API key, the live JSON matches the documented schema field for field;
  - this file is current.
- **Reviewed:** an API change shows up as a diff of this file. Regenerate it with
  `UPDATE_OPENAPI=1 cargo test -p app-api --test openapi` in `backend/`.

| tag | operations |
|---|---|
| runs | list, create, get, delete; daily analytics (analytics module) |
| audit | the organization's audit trail |
| organization | members, teams, roles; the permission catalog (public) |

Each operation lists the scope it needs, for example `runs:create`. A credential's scopes are a
subset of its creator's permissions at the time of each request.

**Not in the document:** sign-in, the session, the account pages, invitations, credential
management, the system admin console and the event streams. They serve the browser through the
cookie session and CSRF protection (the browser-for-frontend, or BFF, model). The SPA calls them
through types that `ts-rs` generates from the same Rust code (`frontend/src/api/generated`), so no
second generated client is needed for it.

## Clients

Working examples in curl, Python and Node.js live in
[`examples/api-clients`](../../examples/api-clients/README.md); the system smoke runs them. To
generate a typed client in your language, point any OpenAPI generator at the document, for
example:

```sh
npx openapi-typescript http://localhost:8080/api/v1/openapi.json -o integration-api.d.ts
```

To browse it, load the file into any OpenAPI viewer (Swagger Editor, Scalar, Bruno, Postman). The
server deliberately ships no documentation UI: it would need third-party scripts that the
application's Content-Security-Policy forbids.
