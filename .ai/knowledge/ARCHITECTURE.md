# Architecture

_One line per major component: name, responsibility and source path. Machine-readable state (profile, enabled modules) lives in `project.json → architecture`. Reasoning lives in `DECISIONS/`. Human docs live in `docs/`._

| Component | Responsibility | Path |
|---|---|---|
| config | typed layered config (defaults → TOML → `APP__` env), `validate()` (auth, cookies, production secrets) | `backend/crates/config` |
| errors | `ApiError` → RFC 9457 problem+json; redaction helpers | `backend/crates/errors` |
| telemetry | tracing (json/pretty), Prometheus recorder | `backend/crates/telemetry` |
| domain | value types: roles, statuses, `Email`, `Slug`, runs, realtime events | `backend/crates/domain` |
| authz | permissions, `OrgAccess` proof type, RBAC authorizer, optional Cedar (feature `cedar`) | `backend/crates/authz` |
| auth | OIDC BFF (PKCE, nonce, state), token cipher, API keys, service JWT verification, IdP admin (ZITADEL) | `backend/crates/auth` |
| db | SQLx repositories (users, sessions, orgs, audit, api keys, notifications, runs, jobs), migrations | `backend/crates/db`, `backend/migrations` |
| api | Axum router, middleware stack, authentication middleware, extractors, routes, DTOs (ts-rs) | `backend/crates/api` |
| networking | outbound API engine: adaptive concurrency, learned rate, breaker, retries, pools | `backend/crates/networking` |
| cache | `CacheLayer` (single-flight, locks, TTL jitter): memory / Redis / Dragonfly; Redis GCRA | `backend/crates/cache` |
| rate_limit | in-process GCRA limiter for inbound requests | `backend/crates/rate_limit` |
| messaging | `EventBus` trait, local bus (PostgreSQL bus in workers) | `backend/crates/messaging` |
| workers | PostgreSQL job queue runtime (SKIP LOCKED, leases, DLQ), `PgEventBus` | `backend/crates/workers` |
| server | API binary (`check-config` subcommand), in-process worker option | `backend/apps/server` |
| worker | standalone job worker binary | `backend/apps/worker` |
| mock-oidc / fake-upstream | test doubles: OIDC provider with faults; simulated external API | `backend/apps/mock-oidc`, `backend/apps/fake-upstream` |
| bench | `app-bench`: outbound and PostgreSQL benchmark scenarios, fixtures, rate trace | `backend/apps/bench` |
| frontend | SolidJS + TS + Vite: AppShell, auth, dashboard, account, org, admin; generated API types | `frontend/` |
| dev CLI | setup/doctor/up/down/check/test/benchmark; validation delegated to `tools/ai-validate` | `dev` |
| benchmarks | harness, gates (invariants + same-machine regressions), reports | `benchmarks/`, `docs/benchmarks/` |
| infra | compose profiles (postgres, cache, dragonfly, identity) | `infra/docker` |
| CI | functional (ai-validate), perf-smoke (invariants), manual benchmark | `.github/workflows` |
| (planned) NATS, ClickHouse, Pingora, Monoio experiment, deployment tiers, generator | see ROADMAP | – |
