# Changelog

All notable changes to the blueprint. Versions are git tags (`vX.Y.Z`). Generated projects record
the version they came from in `.ai/config/project.json → architecture.origin.version` and in
their README.

## Unreleased

Open-source packaging of 1.0.0. The architecture and generated application code are unchanged.

### Added
- **Licensing:** `LICENSE-MIT` and `LICENSE-APACHE` (the dual license `Cargo.toml` already
  declared). Generated projects carry the notices in `third_party_licenses/ultimate-web-stack/`;
  their own license is the owner's choice.
- **Project files:** `CONTRIBUTING.md` (evidence rule, benchmark and architecture-proposal
  workflows), `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1), `SECURITY.md` (private
  reporting), `SUPPORT.md`, `GOVERNANCE.md` and the public `ROADMAP.md`.
- **GitHub:** issue forms (bug, feature, benchmark result, architecture proposal), a PR
  template, `FUNDING.yml` (to be filled in), Dependabot, a label set, and a release workflow that
  publishes this changelog.
- **Docs:** quickstart, why, FAQ, troubleshooting, "what we don't use", AI-agent guide,
  production checklist, versioning policy, and an adding-a-feature walkthrough.
- **Scripts and site:** `scripts/release-audit.sh` (public-release audit: secrets in history and
  tree, personal paths, identities, placeholders), `scripts/demo.sh` (golden-path demo for
  recording), and `site/` (static landing page, not deployed).

### Changed
- **README** rewritten as the project landing page.
- **Generator:** blueprint-only project files stay out of generated projects. A test checks that
  every relative documentation link in a generated project resolves.

### Fixed (found by the first GitHub-hosted CI runs and a clean-clone test)
- **`./dev check` as the first command on a fresh machine:** it now applies migrations. Without
  them, compile-time-checked SQL failed against an empty schema.
- **E2E in CI:** compiles against the committed sqlx metadata, and creates its database over TCP
  when there is no local compose container.
- **Generator:** commits with a neutral identity when git has none configured (fresh CI runners).
- **Intermittent smoke failures:** `cmd | grep -q` under `pipefail` fails on SIGPIPE even when it
  matches. In the image-secret check and the release audit this could have hidden a real match.
  Output is now captured before matching.
- **CI coverage and hardening:** generator and gate self-tests and minimal-feature clippy were
  missing from CI. Third-party actions are now pinned to commit SHAs. The release smoke and the
  live systemd test run as visible steps.

### Fixed (documentation accuracy)
- **Benchmark summary**, checked against the raw tables:
  - keep-alive pooling is 40.9k vs 26.0k req/s (the summary said 35k);
  - the JetStream drain range is 38k–49k jobs/s (the summary mixed in a publish rate);
  - the DB-backed proxy cost is 12–15% with Pingora and 7–8% with nginx (the summary said
    "12–14%, nginx the same").

## 1.0.0 — 2026-10-05

First stable blueprint. Architecture frozen; changes from here on need a failing acceptance test
or new evidence (see the ADR reversal conditions).

### Compatibility
- **AI protocol**: ai-project-template / `ai_protocol_version` 1.2.0 (`tools/ai-init --derive` for
  generated projects).
- **Toolchains**: Rust 1.99.0 (pinned), Node ≥ 22, Python ≥ 3.11, Docker with compose.
- **Services**:
  - PostgreSQL 18;
  - optional: Redis 8, NATS 2.14.7, ClickHouse 26.3.25.2;
  - ZITADEL v4.19.4 for local identity;
  - observability: Prometheus v3.15.0, Tempo 3.1.0, Loki 3.7.8, Alloy v1.20.1, Grafana 13.2.3.
- **Benchmark environment** for every recorded number: Apple M5 (10 cores, 16 GB), macOS 26.5.1,
  colima 0.10.3 VM with 2 CPU / 4 GiB; 2026-10-04 and 2026-10-05.

### Included
- **Core:**
  - SolidJS + TypeScript + Vite SPA;
  - Rust (Tokio, axum, SQLx) API and worker;
  - PostgreSQL as the single source of truth.
- **Identity and access:**
  - OIDC BFF with cookie sessions, CSRF and MFA step-up;
  - organizations, invitations, teams and custom roles;
  - RBAC or Cedar, with an `OrgAccess` proof type;
  - API keys and service accounts;
  - an append-only audit trail;
  - a separate system admin and auditor trust level.
- **Dashboards:** user, organization and system admin.
- **Platform:**
  - PostgreSQL job queue;
  - realtime (SSE and WebSocket);
  - an outbound API engine (learned rate, adaptive concurrency, breaker);
  - W3C tracing, Prometheus metrics, an internal ops port.
- **Optional modules** (cargo feature + config + compose + CI + validation command):
  - Redis cache;
  - NATS/JetStream;
  - ClickHouse analytics;
  - the observability stack;
  - the Pingora gateway.
- **Deployment tiers:** a distroless image with production compose, systemd + Caddy, and a
  Kubernetes base.
- **Generator and validator:** `scripts/create-project`, `scripts/validate-generated`.
- **Evidence:**
  - the benchmark harness with gates;
  - the system smoke test, browser acceptance and failure drill;
  - the live systemd test and release smoke;
  - ADRs 0003–0011.

### Decided by measurement
See `docs/benchmarks/SUMMARY.md`.
- **Default:** split outbound controllers, the PostgreSQL queue, the in-process cache, Tokio +
  axum, thin LTO, trace propagation.
- **Profile-specific:** NATS, ClickHouse, Redis, Pingora.
- **Inconclusive:** PGO, mimalloc, jemalloc, thread-per-core Tokio, Dragonfly.
- **Rejected:** Monoio/io_uring, a default proxy hop, a Redis session cache, fat LTO by default.

### Fixed during release acceptance
- **Authorization:**
  - an empty organization update ran without an authorization decision;
  - escalation and credential-scope denials were not audited;
  - organization reads now share the engine path.
- **Logout:** it could race the CSRF token and leave the user signed in.
- **Worker crash-loop:** under systemd and Kubernetes, `APP__WORKER__PORT` was rejected. It is
  now the `worker.port` config key, and every deployment `APP__` variable is checked by a test.
- **Readiness:** PostgreSQL shutdown codes and protocol errors now return 503 instead of 500;
  NATS outages now show in readiness.
- **Reproducibility:**
  - E2E runs beside a running dev stack;
  - `./dev check` starts the services its commands need.
- **Generator:**
  - failure cleanup;
  - no source-machine paths;
  - no inferred placeholder commands;
  - unselected modules leave no CI services.
