# Constraints

_Hard constraints: platform, compliance, performance, compatibility and security. Each entry gives the constraint, its source (a user, a contract or a law), and how it is verified._

| Constraint | Source | Verified by |
|---|---|---|
| Permanent core: SolidJS + TypeScript frontend; Rust (Axum, Tokio, Serde, Hyper/Reqwest) backend; PostgreSQL via SQLx. Everything else is an optional module. | user (mission brief 2026-10-04) | `project.json → architecture.components`; ADR 0010 (Tokio + axum, measured) |
| Optional infrastructure (Redis/Dragonfly, NATS/JetStream, ClickHouse, Pingora, specialised runtimes, ScyllaDB, Kubernetes) activates only by profile and only when evidence justifies it. A core project must run with PostgreSQL alone. | user | `./dev up` for the core profile starts only PostgreSQL; core-profile tests |
| Modular monolith first; services are extractable later, never split for their own sake. | user | crate boundaries in `backend/crates/` (repositories take `&OrgAccess`; no cross-crate SQL) |
| No performance claim without our own measurement. Never fabricate benchmark results or service connections. External benchmarks (TechEmpower) are context, not authority. | user | `benchmarks/results/*.json` carry environment + git provenance; `docs/benchmarks/` generated from them |
| Optimise successful useful work per dollar per second, not raw requests/second. | user | benchmark reports include success rps, error rates and per-core / per-GB figures |
| PostgreSQL is the transactional source of truth; caches and ClickHouse never become canonical. | user | cache paths are read-through; analytics is append-only; ADR 0004 (no Redis session cache), ADR 0008 (ClickHouse) |
| Never commit secrets; only `.env.example`. | user + AI protocol | `ai-check` secret patterns; `./dev check` secret scan |
| The AI operating protocol is `AI_PROTOCOL.md` with `.ai/` and `tools/ai-*` (ai-project-template, protocol 1.2.0); do not create a competing state system. Projects generated from the blueprint get a fresh instance (`tools/ai-init --derive`). | user | `python3 tools/ai-check`; generator tests assert it passes in generated projects |
| No Node.js in production unless SSR requirements justify it. | user | release image `infra/docker/Dockerfile`: Rust binaries + static files on distroless |
| Kubernetes is never the default deployment. | user | `docs/deployment/README.md` (VPS and single-host containers come first; Kubernetes is a tier, not the default) |
| Do not push to remotes unless authorised. | user | — |

## Identity and security invariants (user, 2026-10-04: permanent)
Verified by the auth/tenancy test suites (`backend/crates/api/tests/`) and `docs/authorization/`.

1. Authentication is not authorization.
2. Frontend visibility is never authorization.
3. The Rust backend is the final application authorization boundary.
4. Browser secrets (tokens, refresh tokens, credentials) are never stored in localStorage/sessionStorage; browsers hold only an HttpOnly session cookie.
5. Client-supplied tenant/organization ids are never trusted without a server-side membership/permission check.
6. Identity-provider data and application data have explicitly documented ownership (`docs/authentication/data-ownership.md`).
7. Default authorization behaviour is deny; uncertainty denies; never fail open.
8. Every privilege-escalation path is tested.
9. System administrators and organization administrators are separate trust levels; a tenant admin never gains system access.
10. Security-sensitive operations create audit evidence.

| Constraint | Source | Verified by |
|---|---|---|
| Default identity provider is ZITADEL (self-hosted or Cloud) via standard OIDC/OAuth2/PKCE behind an internal abstraction; business logic never calls ZITADEL-specific APIs directly. | user | `app-auth` traits; mock-OIDC conformance tests |
| Browser apps use the BFF model: the Rust backend owns tokens and sessions. | user | auth flow tests |
| Do not implement auth cryptography; use vetted libraries (OIDC client, JWT/JWK, AEAD, HMAC). Passwords belong to the IdP. | user | dependency list; ADR |
| Default full blueprint includes ZITADEL auth, BFF sessions, RBAC, user dashboard, account center, admin dashboard, audit logging and security tests; Redis/NATS/ClickHouse/Pingora/Cedar remain profile-driven. | user | generator defaults; `project.json → architecture` |
