# Authentication

Authentication answers *who is calling*. It never answers *what they may do*; that is
[authorization](../authorization/README.md) (invariant 1).

## Model: Backend-for-Frontend (BFF)

The Rust backend is the OpenID Connect client. The browser never holds an access, refresh or
ID token: it holds one opaque, HttpOnly session cookie (invariant 4). The identity provider (IdP)
handles credentials, passkeys, MFA, social and enterprise login. The application issues no
credentials and has no password storage.

```mermaid
sequenceDiagram
    autonumber
    participant B as Browser (SolidJS)
    participant A as Rust API (BFF)
    participant I as IdP (ZITADEL / any OIDC)
    participant D as PostgreSQL
    B->>A: GET /auth/login?return_to=/dashboard
    A->>D: store flow {state hash, nonce, PKCE verifier (AES-GCM encrypted), return_to}
    A-->>B: 302 to IdP authorize (code + PKCE S256 + state + nonce)<br/>Set-Cookie: flow binding (HttpOnly, short-lived)
    B->>I: authenticate (passkey / password+MFA / Google / Apple / SAML)
    I-->>B: 302 /auth/callback?code&state
    B->>A: GET /auth/callback (flow cookie)
    A->>D: consume flow (single use; state must match the browser's flow cookie)
    A->>I: token exchange (code + verifier) — server to server
    A->>A: verify ID token: signature (JWKS), issuer, audience (+trusted list), nonce, expiry
    A->>D: upsert user by (issuer, sub); create session (token stored as SHA-256)
    A-->>B: 302 return_to (same-origin path only)<br/>Set-Cookie: __Host-app_session (HttpOnly; Secure; SameSite=Lax; Path=/)
    B->>A: GET /api/v1/session (cookie)
    A-->>B: user, csrf_token, organizations, features
```

## Principals

Each request resolves to exactly one principal (`backend/crates/api/src/auth/mod.rs`):

| credential | principal | used by |
|---|---|---|
| `Authorization: Bearer <prefix>_<env>_<id>_<secret>` | API key, bound to one organisation | scripts, integrations |
| `Authorization: Bearer <JWT>` | service account (M2M), verified with the IdP's JWKS against an algorithm allowlist; must be registered with an organisation | backend services |
| session cookie | user | the browser |

A bearer credential that is present but invalid is a hard 401. The request never falls back to
the cookie.

## Sessions

| property | value (default) | config |
|---|---|---|
| token | 32 random bytes, base64url (43 chars); stored only as SHA-256 | – |
| cookie | `__Host-app_session` when `cookie_secure` (required in production), HttpOnly, SameSite=Lax, Path=/ | `auth.cookie_*` |
| absolute lifetime | 7 days | `auth.session_absolute_ttl_minutes` |
| idle timeout | 12 hours (sliding; `last_seen_at` written at most once a minute) | `auth.session_idle_ttl_minutes` |
| rotation | every 60 min, on safe methods only; the previous token stays valid for 60 s for in-flight requests | `auth.session_rotate_minutes` |
| recent-auth window | 10 min (account deletion, sensitive admin actions) | `auth.reauth_window_minutes` |
| metadata | IP (optional), user agent (optional), `amr`, MFA flag, auth time | `auth.store_client_ip`, `auth.store_user_agent` |

Revocation covers logout, logout-everywhere, revoking one device, user suspension and admin
revoke. It is immediate, because every request reads the session from PostgreSQL. There is no
session cache. See [ADR 0004](../../.ai/knowledge/DECISIONS/0004-no-redis-session-cache.md) for
the decision, with benchmark evidence. Redis is never the source of session truth.

Logout revokes the session and redirects to the IdP's `end_session_endpoint`, with the ID
token as `id_token_hint`. That token is kept encrypted with AES-256-GCM, using the session id as
associated data, and is used only for this purpose.

## CSRF

There are two independent layers:
1. **Fetch Metadata / Origin** (tower-http `CsrfLayer`) rejects cross-site unsafe requests.
2. **Per-session token**: cookie-authenticated unsafe requests must send `X-CSRF-Token`, compared
   in constant time. The SPA gets the token from `/api/v1/session` and keeps it in memory only.

Bearer-authenticated requests are exempt, because browsers never attach bearer tokens
automatically.

## Login UX

`/login` follows the brief's hierarchy: passkey first, then Google and Apple when configured,
then "or", email + Continue, and finally "Already registered?". The UI is branded and
accessible. All buttons start the same server-side flow with different `prompt` / `login_hint` /
IdP hints; the IdP renders the credential step. Registration (`/register`) uses `prompt=create`.

## Identity providers

- **ZITADEL** (default, self-hosted or Cloud): `infra/docker/compose.yaml` profile `identity`, with
  `scripts/zitadel_bootstrap.py` for project, app, roles and dev users. Its project id is
  accepted as an extra ID-token audience (`auth.id_token_trusted_audiences`). System roles can
  come from `urn:zitadel:iam:org:project:roles`.
- **Any OIDC provider**: `auth.provider = "oidc"`. Passkey and MFA management then links to
  `auth.account_console_url`.
- **Local tests**: `backend/apps/mock-oidc`, a real OIDC provider with fault injection: expired,
  wrong audience, wrong issuer, bad signature, wrong nonce, missing email, extra audience.

## Data ownership

| data | owner | notes |
|---|---|---|
| credentials, passkeys, MFA factors, social/SAML links, password reset, email verification | IdP | never stored by the app |
| `users.identity_provider`, `users.external_subject` | IdP (the identity key) | an exact issuer + `sub`; email is **not** an identity key |
| `users.email`, `email_verified` | IdP, cached at each login | the IdP is the source of truth |
| `display_name`, `avatar_url`, `preferences` | application | |
| `system_role` | application (or the IdP when `system_roles_from_idp`) | |
| organisations, memberships, roles, teams, invitations, API keys, sessions, audit | application | |

## Security tests

These run in `backend/crates/api/tests/auth_flow.rs` and `credentials.rs`, through the real router
and real PostgreSQL. They cover:
- tokens never reach the browser, and cookie attributes are correct;
- forged, expired and revoked sessions, and CSRF on writes;
- invalid ID tokens (each fault) and untrusted extra audiences;
- login state binding and single use, and `return_to` open-redirect attempts;
- PKCE and prompt parameters;
- suspended users;
- logout, logout-everywhere and rotation;
- MFA detection from `amr`;
- the API key lifecycle, scope and creator bounds, and service-account JWTs (including
  `alg=none`).

End-to-end coverage against real ZITADEL is in `frontend/tests/zitadel/` (`./dev test --zitadel`).
