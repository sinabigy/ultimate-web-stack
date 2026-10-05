# Ultimate Web Stack Blueprint 1.0.0: final report

A reusable, evidence-driven foundation for secure web products. One tested command generates an
independent application with:
- authentication and dashboards;
- tenant-safe authorization;
- PostgreSQL;
- the AI project protocol;
- only the infrastructure its profile requires.

Every technology choice is backed by a measurement or a test. Rejected and inconclusive options
are recorded too.

Acceptance evidence: [FINAL_ACCEPTANCE.md](FINAL_ACCEPTANCE.md). Benchmark decisions:
[benchmarks/SUMMARY.md](benchmarks/SUMMARY.md). Decisions: `.ai/knowledge/DECISIONS/`
(ADRs 0003–0011).

## Golden commands

Every command below was run during release acceptance (see FINAL_ACCEPTANCE.md).

```sh
# Create (from the blueprint repository; DEST must be outside it)
scripts/create-project ../my-product --name "My Product"                        # core profile, b2b auth
scripts/create-project ../my-saas --name "My SaaS" --profile performance --cedar   # + Redis cache, Cedar
scripts/create-project ../my-scale --name "My Scale" --profile distributed        # + NATS, ClickHouse

# Enter
cd ../my-product

# Validate the AI/project state
python3 tools/ai-check

# Run locally (http://localhost:5190, mock identity provider: any email)
./dev setup
./dev up

# Validate everything (lint, audit, tests, E2E, deployment tiers, release image)
./dev check

# Live proofs against the running stack
./dev test --system      # one journey through every component
./dev test --journey     # browser acceptance with screenshots
./dev test --drill       # failure drill (stops services briefly)

# Stop
./dev down
```

## What we built
- **Product**: a SolidJS SPA and a Rust API plus worker, with the identity and tenancy layer
  most products need on day one:
  - OIDC BFF login, sessions, MFA step-up;
  - organizations, members, invitations, teams and custom roles;
  - API keys and service accounts;
  - an append-only audit trail;
  - user, organization and system-admin dashboards.
- **Platform**:
  - background jobs;
  - realtime (SSE and WebSocket);
  - an outbound API engine that learns provider limits;
  - W3C tracing end to end, metrics on an internal ops port, JSON logs.
- **Evidence machinery**:
  - a benchmark harness with invariant and regression gates;
  - a system smoke test, browser acceptance, a failure drill;
  - a live systemd test and a release-image smoke test.
- **Reuse machinery**:
  - `scripts/create-project` (the generator) and `scripts/validate-generated` (proof that a
    generated project works on its own);
  - the AI working protocol (ai-project-template 1.2.0).

## Universal core
Present in every generated project and not optional:

| layer | choice | evidence |
|---|---|---|
| frontend | SolidJS + TypeScript + Vite, generated API types (ts-rs) | bundle budget gate; a11y and CSP E2E |
| backend | Rust, Tokio + axum, SQLx (compile-time checked SQL) | ADR 0010 (Tokio measured fastest; axum overhead > any runtime difference) |
| database | PostgreSQL 18: the single source of truth for sessions, jobs, events and audit | ADR 0004, ADR 0007 |
| identity | OIDC BFF (PKCE, nonce, state), HttpOnly cookie sessions, CSRF tokens | auth tests, E2E, ZITADEL live test (blueprint) |
| authorization | RBAC (default) or Cedar, `OrgAccess` proof type, default deny | authorization matrix under both engines |
| jobs | PostgreSQL queue (`SKIP LOCKED`, leases, DLQ, idempotent enqueue) | 4.4k–5.1k jobs/s; ADR 0007 |
| outbound | rate and concurrency controllers, breaker, pooling | 496–500 useful req/s against a 500 req/s limit; ADR 0005 |
| observability | tracing + OTLP (opt-in export), Prometheus metrics, JSON logs | +1.46 µs per request for propagation; ADR 0006 |

## Optional profiles

| profile | adds | when (measured) |
|---|---|---|
| `core` (default) | – | one host; PostgreSQL alone |
| `performance` | Redis cache and shared rate limits (`cache`) | several API instances; on one instance, in-process is 2.2× faster |
| `distributed` | + NATS/JetStream (`messaging_nats`), ClickHouse (`analytics_clickhouse`) | about 10× the PG queue drain; about 24× faster cross-tenant aggregates |
| `full` | + Prometheus, Tempo, Loki, Alloy, Grafana dev stack | observability development |
| `--with-gateway` | Pingora edge workspace | programmable edge logic only; as a plain proxy it equals nginx and costs a hop |

Each optional module is a cargo feature, plus configuration, plus a compose profile, a CI service
and a validation command. An unselected module is absent from all five, and the server's
startup `configuration` log line proves what is active. To switch a module on later, see
[MODULES.md](MODULES.md).

## Authentication
- BFF pattern: the browser holds only an HttpOnly, `__Host-` cookie. Tokens stay server-side,
  encrypted with AES-256-GCM (RustCrypto `aes-gcm`; no home-made cryptography). Nothing sensitive goes in localStorage.
- Supported flows: registration, login, logout, email verification and recovery (IdP-owned),
  MFA and passkeys (IdP), step-up re-authentication, session list and revocation, account
  deletion.
- Identity providers:
  - **mock OIDC** for development and tests;
  - **self-hosted ZITADEL**, verified live in the blueprint (hosted login, roles, MFA step-up,
    logout, M2M);
  - generic OIDC providers, configuration only, **not verified live**.

## Authorization
- Every org-scoped handler decides through the configured engine:
  - `require` for writes, which audits denials;
  - `require_read` for reads;
  - `deny` for structural checks such as escalation and credential scopes.
- Repositories accept only an `OrgAccess` proof, never an organization id from the client.
- A non-member gets 404 (existence is not revealed). System admin and system auditor are a
  separate trust level, MFA-gated.
- The authorization matrix covers: anonymous, member, admin action, cross-tenant, org admin,
  org admin → platform, auditor (read allowed, mutation denied), system admin. It runs over HTTP
  under **RBAC and Cedar**, with audit counts. A Cedar project policy is proven to apply to reads
  over HTTP.

## Dashboard foundation
SolidJS shell with a command palette, light and dark themes, and accessibility checks (axe). It
has three areas:
- **user**: dashboard, profile, security, MFA, passkeys, sessions, notifications, activity;
- **organization**: overview with charts, runs, members, teams, roles, API keys, audit, settings,
  billing;
- **system admin**: overview, users, organizations, roles, jobs, audit, providers, health.

Browser acceptance screenshots every one of them, per role.

## AI protocol integration
The blueprint *is* an ai-project-template 1.2.0 instance: `AI_PROTOCOL.md`, `.ai/` state and
tasks, and `tools/ai-*`. There is no competing state system. `create-project` starts a fresh,
derived instance:
- task history is reset;
- knowledge, constraints, ADRs and conventions are inherited;
- provenance is recorded in `project.json`.

Two clean-room runs (agents given only a generated repository) built complete secure features
from repository-local information alone.

## Generator
`scripts/create-project` takes:
- profiles `core | performance | distributed | full`;
- auth profiles `basic | consumer | b2b | enterprise`;
- `--organizations`, `--admin`, `--cedar`, `--modules`, `--with-gateway`.

It runs non-interactively from flags, or interactively on a terminal. It refuses invalid
combinations, a non-empty destination, and destinations inside the blueprint. On failure it
removes the partial destination. Generated repositories contain no source-machine paths (tested).
`scripts/validate-generated` proves a generated project end to end:
- protocol, setup and services;
- `./dev check`, live system smoke and runtime configuration;
- absence of unselected modules from binaries, services and configuration.

## Benchmark results
Machine: Apple M5 (10 cores, 16 GB), services in a 2-CPU / 4 GiB colima VM. Linux experiments ran
in containers on that VM. Method: median of 3, measured noise ≤ 7.6% between full runs. Full
tables: [benchmarks/SUMMARY.md](benchmarks/SUMMARY.md).

| area | DEFAULT | PROFILE-SPECIFIC | OPTIONAL / INCONCLUSIVE | REJECTED |
|---|---|---|---|---|
| outbound engine | split rate/concurrency controllers, pooling, breaker | – | – | naive retrying client (9.3% completed, 99.5% waste) |
| job queue | PostgreSQL | NATS JetStream (35k–49k vs 4.4k–5.1k jobs/s) | – | – |
| analytics | PostgreSQL per tenant (5.5–6.4 ms) | ClickHouse cross-tenant (24 ms vs 573 ms) | – | – |
| cache | in-process (125k req/s) | Redis across instances (57k) | Dragonfly (54k) | Redis session cache |
| edge | Axum behind Caddy or nginx TLS | Pingora for edge logic | – | extra proxy hop for speed (143k → 66k req/s) |
| runtime | Tokio + axum | – | thread-per-core Tokio (+3–7% raw loops) | Monoio / io_uring (0–24% slower than epoll) |
| allocator | system | – | mimalloc, jemalloc (within ±10% noise) | – |
| LTO | thin, codegen-units 1 | – | – | fat by default (−14% size, +38% build, no speed) |
| PGO | off | – | +7 to +16%, inside the 19% spread | – |

## Technologies retained
SolidJS, TypeScript, Vite, Rust, Tokio, axum, SQLx, PostgreSQL 18, reqwest (pooled), Cedar
(optional), Redis 8 (optional), NATS 2.14 with JetStream (optional), ClickHouse 26.3 (optional),
OpenTelemetry/OTLP, Prometheus, Tempo, Loki, Alloy, Grafana (dev observability), ZITADEL v4
(identity provider), Pingora 0.9 (optional edge), distroless images, systemd + Caddy, Kubernetes
base manifests.

## Technologies rejected
Monoio / io_uring runtime, Pingora (or any extra hop) in the default request path, Redis session
cache, fat LTO by default. Not adopted for lack of a reliable win: PGO, mimalloc, jemalloc,
thread-per-core Tokio, Dragonfly over Redis.

## Security findings fixed
Each was found by a test, a benchmark, a live run or a clean-room agent, and each now has a
regression test:
- **Rate limiting:** limits were keyed by IP and applied before authentication. They are now per
  principal (benchmark).
- **Ops endpoints:**
  - `/metrics` and detailed `/readyz` were public. They are now on an internal ops port, and the
    public `/metrics` is an explicit 404 (review and smoke).
  - Ops endpoints were rate-limited and shed under load (benchmark).
- **Audit gaps:**
  - a member's forbidden organization update was not audited (authorization matrix);
  - escalation and credential-scope denials were not audited (release acceptance scan).
- **Empty `PATCH /orgs/{slug}`:** it wrote and audited without any authorization decision. It is
  now denied by default (browser acceptance).
- **Logout CSRF race:** logout could post before the CSRF token loaded, leaving the user signed in
  (clean-room run 1, journey).
- **Session hardening:** two security-header and auth bugs (browser E2E), and three ZITADEL
  integration issues (live ZITADEL).
- **Heartbeat leases** survived cancelled job handlers (mutation-checked test).

Deployment and correctness fixes found the same way:
- **Worker crash-loop on systemd and Kubernetes:** `APP__WORKER__PORT` had no config key. Found
  by the live systemd test; now guarded by a test of every `APP__` variable in deployment files.
- **Database outages answered 500, not 503:** admin shutdown codes and protocol errors were
  misclassified (failure drill).
- **NATS outages were invisible to readiness** (failure drill).
- **Leaked machine paths:** generated READMEs and CONSTRAINTS.md pointed at source-machine paths
  (generator acceptance).

## Clean-room findings
Two agents were each given only a freshly generated repository and a product request.

| | run 1 | run 2 (release candidate) |
|---|---|---|
| project | `performance`, `b2b`, Cedar | default `core`, `b2b` |
| feature | organization Projects (CRUD, permissions, audit, UI) | organization Announcements with member notifications |
| `./dev check` | 23/23 | 22/22 (includes release smoke and live systemd) |
| live proof | member 403, cross-tenant 404 | member writes 403 (audited), cross-tenant 404 by slug and id, notifications only to the other members |
| looked outside the repo | no | no |

Run 2 scorecard (all **yes**):
1. discover architecture;
2. retrieve knowledge;
3. authorization conventions;
4. database state;
5. backend API;
6. frontend;
7. tenant isolation;
8. audit;
9. tests;
10. validation;
11. durable AI knowledge.

The authorization recipe and `CONVENTIONS.md` were named as the decisive documents.

Reusable gaps found, all fixed in the blueprint:
- **Run 1:**
  - the logout CSRF race (a real bug);
  - E2E collided with a running dev stack;
  - two authorization call paths with no guidance;
  - no "add a permission" guide;
  - an empty conventions file;
  - the sqlx `DATABASE_URL` workflow undocumented;
  - stale constraints and architecture entries;
  - an inferred placeholder validation command.
- **Run 2:**
  - `./dev test --journey` needed an undocumented env var (now set by `./dev up` with the mock
    IdP);
  - no guidance for a feature request arriving before discovery;
  - the notification system undocumented;
  - a hand-maintained read-permission list in `deny()` (now derived from the `:read` suffix);
  - org navigation in three places undocumented;
  - `./dev check` duration and VM memory undocumented.

Not fixed in the blueprint, because they are protocol-tool issues that belong to
ai-project-template:
- the result-packet `lessons` shape is not shown in `docs/protocol/task-lifecycle.md`;
- `ai-validate` output is buffered when redirected.

## Deployment options
| tier | artefact | verified |
|---|---|---|
| VPS | release binaries + systemd units (hardened) + Caddy | **live**: systemd in a Debian 12 container, covering start, check-config, migrations, non-root, worker, restart, graceful stop, crash restart (`infra/systemd/live-test.sh`) |
| containers | distroless release image + production compose | **live**: release smoke covering health, readiness, public surface, non-root, read-only, no baked secrets, restart, graceful shutdown (`infra/docker/smoke.sh`) |
| Kubernetes | base manifests + kustomize (HPA, PDB, NetworkPolicy, probes on the ops port) | **static only**: kubeconform `-strict`; **not run on a live cluster** |
| hyperscale edge | Pingora gateway | compile, lint and benchmark only |

## How to create a new project
Run `scripts/create-project` (see the golden commands), then `./dev setup && ./dev up`, and sign
in with any email on the mock identity provider. Choose the profile from the measured "when" in
the table above. Add modules later with [MODULES.md](MODULES.md).

## How an AI starts work
Inside a generated project:
1. Read `AI_PROTOCOL.md` (short), then `.ai/config/project.json` (objective, stack, modules,
   validation commands) and `.ai/state/CURRENT_TASK.json`.
2. Run `python3 tools/ai-check`, then `python3 tools/ai-task list`. The first task is discovery:
   confirm the objective with the user.
3. Before changing code, read `.ai/knowledge/CONVENTIONS.md` (workflow rules) and, for endpoints,
   the guide "Adding a permission and protecting a new endpoint" in
   `docs/authorization/README.md`.
4. Validate with `./dev check`, record the evidence with `tools/ai-task complete`, and keep
   `.ai/knowledge/` current.

## Scaling path
1. One host: VPS with systemd and Caddy, or the production compose.
2. Several API instances: the `cache` module (Redis); the PostgreSQL event bus already fans out.
3. High job volume or cross-service consumers: `messaging_nats`.
4. Cross-tenant analytics: `analytics_clickhouse`.
5. Many replicas: the Kubernetes base.
6. Programmable edge: the Pingora gateway on dedicated nodes.

## Known limitations
- **Not verified live:**
  - Kubernetes (static validation only);
  - ZITADEL Cloud and generic OIDC providers (configuration and discovery check only; self-hosted
    ZITADEL was verified live in the blueprint);
  - GitHub CI (nothing pushed);
  - TLS issuance (Caddy configuration validated, not exercised).
- **One machine:** every number comes from one Apple M5 host, with services in a 2-CPU / 4 GiB VM.
  HTTP numbers are lower bounds, and results do not transfer to other hardware without
  re-measuring.
- **Still undecided:** PGO, alternative allocators and thread-per-core Tokio need dedicated Linux
  hardware.
- **Intermittent on this host:**
  - `sqlx::test` connect failures through the colima port forwarder (pass on rerun);
  - one historical E2E flake under full load.
- **Cosmetic:** an empty "Job queues" chart card when no jobs are queued, and an empty
  organization switcher on not-found pages.
- **Delegated to the IdP:** email delivery, password policy and MFA enrolment (simulated by the
  mock IdP in development).
- **Not covered by the failure drill:** network partitions between API instances, and disk-full on
  the database host.
- **Upstream protocol-tool improvements** (ai-project-template, not this blueprint): document the
  result-packet `lessons` shape, and stream `ai-validate` output when redirected.

## Final validation results
All on the environment in [FINAL_ACCEPTANCE.md](FINAL_ACCEPTANCE.md#environment), 2026-10-05.

| proof | result |
|---|---|
| blueprint `./dev check` on the release tree (tag `v1.0.0`; 05:13–05:18 UTC) | **29 pass, 0 fail, 0 skip**: fmt, clippy (full and minimal features), gateway, sqlx offline data, cargo audit and deny, npm audit, gitleaks, config check, Rust tests, Cedar, cache (Redis + Dragonfly), NATS, ClickHouse, gate and generator self-tests, TS bindings, frontend typecheck, lint, unit, build and bundle budget, E2E, deployment-tier verification, release smoke, live systemd |
| tests in the suite | 294 Rust tests (all features), 14 frontend unit, 11 E2E, 6 browser acceptance, 5 generator, 6 gate self-tests |
| authorization matrix | passes under RBAC and Cedar, with audit counts; Cedar read-policy test mutation-checked |
| browser acceptance (`./dev test --journey`) | 6/6 per role against the full stack; 27 screenshots |
| failure drill (`./dev test --drill`) | 14/14 |
| live systemd test / release smoke | pass / pass (no secrets in image, restart, graceful SIGTERM) |
| benchmark harness (`./dev benchmark --smoke`, `9efafc8`) | 17/17 invariant gates |
| generated projects (fresh, from the release candidate) | minimal 22/22, SaaS 24/24, throughput 25/25 `./dev check`, plus live smoke and module checks; regeneration from the final commit differs only in provenance stamps |
| clean-room runs | 2/2 features built from repository-local information; 23/23 and 22/22 |

Failures and retries during release, all recorded:
- **Release-candidate runs:**
  - `generator-selftest` failed twice; both times the path-leak guard caught an absolute path
    in my own acceptance documents, which were then fixed;
  - `rust-test-cache` failed once because Redis and Dragonfly were not running, so `./dev check`
    now starts them;
  - the benchmark smoke failed once because PostgreSQL was stopped, so `./dev benchmark` now
    starts it.
- **One intermittent failure in the final tree's first run:** `rate_limited_provider_is_respected_and_work_completes`
  exceeded its 20 s bound, against a normal 5 s. Not reproduced in 14 runs; the assertion now
  reports its diagnostics, and it is recorded in `KNOWN_ISSUES.md`. The immediate rerun of the
  full check passed 29/29.
- **The validator's runtime-configuration step** was re-evaluated from saved server logs after a
  parser bug in the validator was fixed (`acceptance/generated-projects.md`).
