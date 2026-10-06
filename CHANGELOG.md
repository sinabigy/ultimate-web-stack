# Changelog

All notable changes to the blueprint. Versions are git tags (`vX.Y.Z`). Generated projects record
the version they came from in `.ai/config/project.json → architecture.origin.version` and in
their README.

## Unreleased

- **Projects run side by side.** Before this, a second project's `./dev up` failed halfway with
  Docker's `port is already allocated` and left a container without ports. Its readiness probe could
  also be answered by another project's server on the same port.
  - Every development port is declared once, in `infra/dev-ports.env`, and read by `./dev`,
    compose, Vite, Playwright, the validation commands, the smoke scripts and the benchmarks.
  - `create-project` gives each project its own block of consecutive ports (20000–29999, derived
    from the name; `--port-base N` picks one). The blueprint keeps its documented ports.
  - `./dev up`, `check`, `test` and `benchmark` check every port before starting anything. They
    name what holds a busy port (a container, or a process with its directory) and say how to
    move. `./dev ports` lists the ports and their holders.
  - Readiness waits on the process `./dev up` started: it fails fast if that process exits, and
    checks the listener's PID where `lsof` exists.
  - The release-smoke and systemd tests use project-scoped container and image names, so two
    projects can validate at the same time.
  - The dev session cookie is named after the project, so signing in to one local project no
    longer signs you out of another (browsers keep cookies per host, not per port).
  - **Projects generated from 1.0.x** keep their current ports. To adopt this, copy the files
    named in the pull request and give `infra/dev-ports.env` a free block.
- **Compression and HTTP caching** ([ADR 0012](.ai/knowledge/DECISIONS/0012-compression-and-http-caching.md)).
  - `npm run build` writes brotli-11 and gzip-9 copies of static files. The server already
    negotiated them, but the build never produced them, so the SPA went out uncompressed without a
    compressing proxy. First-load JS/CSS drops from 186 KB to 53 KB.
  - Source maps move out of `dist/` into `frontend/sourcemaps/`. They were served publicly.
  - API responses from 1 KiB are compressed on the fly (brotli 4 / gzip 6; `http.compression`,
    on by default). A real 13 KB list page goes from 13,020 to 1,329 bytes. Throughput stays
    within the run-to-run spread (`benchmarks/run.py --suite compression`).
  - Never compressed: `no-store` responses, which carry credentials (BREACH), event streams, and
    already-encoded files.
  - API responses default to `Cache-Control: private, no-cache`, so a CDN or proxy never stores
    per-user data.
  - API ETags are rejected for now: the SPA is push-driven and does not poll. The ADR records when
    to add them.
- **OpenAPI for integrations.** `GET /api/v1/openapi.json` (and `docs/api/openapi.json`) describes
  the 10 operations that API keys and service accounts use, with the scope each one needs.
  - Schemas derive from the Rust response types (utoipa).
  - A test proves that every documented operation is a real route and refuses anonymous requests,
    and that live responses match the schemas field for field. It also fails when a new route is
    neither documented nor marked browser-only.
  - The SPA keeps its `ts-rs` types; no second generated client.
- **Alert rules.** The production checklist asked for alerts, but the blueprint shipped no rules.
  `infra/docker/observability/alerts.yml` now has 12 symptom-based rules with a runbook each
  ([alerting](docs/operations/alerting.md)). `promtool` unit tests prove each rule fires on its
  symptom and stays quiet on healthy or low traffic; they run in `infra-verify`. The
  observability stack loads the rules. No Alertmanager: routing alerts to people is a deployment
  choice. Tail sampling is documented as a production-collector setting rather than shipped.
- **Integration examples.** `examples/api-clients/` has the same API-key client in curl, Python and
  Node.js: list runs, create one, wait for it, and handle RFC 9457 errors. `./dev test --system`
  runs all three with a freshly scoped key, then revokes it and checks the revoked key gets 401.

Maintenance on `main`, with no release planned for these alone.
- **Test reliability:** the GCRA property tests compute granted slots from the limiter's own clock
  reading. Under parallel load the old measurement failed 13 of 40 runs ([KNOWN_ISSUES](.ai/knowledge/KNOWN_ISSUES.md)).
  The bound is tighter, not looser, and the limiter is unchanged.
- **`./dev benchmark`** passes options through to `benchmarks/run.py` (`--suite http --repeat 5`).
  A partial run skips the gates and the report, which describe complete runs.
- **Release audit:** the launch-placeholder patterns live in one file
  (`scripts/launch-placeholders.txt`), shared by the audit and the Pages workflow's guard. The guard
  no longer triggers a false-positive audit warning. A regression test covers both behaviours.
- **Diagnostics:** the `rust-test*` commands merge stderr, so `ai-validate`'s output tail names a
  failing test. The system smoke names the URL of a connection failure. Both target the transient
  failures recorded in `KNOWN_ISSUES.md`; there are no retries.

## 1.0.2 — 2026-10-06

Security and maintenance release. The main reason to upgrade is the patched `seroval`
resolution. The architecture and defaults are unchanged.

- **Security:** `seroval` and `seroval-plugins` are forced to 1.6.8 through npm `overrides`.
  - This fixes two critical advisories, GHSA-p6vx-979v-rg4c and GHSA-jp82-f5mq-hwhp.
  - Every `solid-js` release still pins `seroval ~1.5`, so the override is needed.
  - The shipped SPA bundle never contained `seroval` (no server rendering), so deployed apps were
    not exposed. The build-time dependency tree and the `npm audit` gate were affected.
  - **Projects generated from 1.0.0 or 1.0.1:** add the same `overrides` block to `frontend/package.json`
    and run `npm install`.
- **Dependencies:** `base64` 0.22 → 0.23 for the project's own crates, after review:
  - no behaviour change for the `STANDARD` and `URL_SAFE_NO_PAD` engines the project uses;
  - 0.23 was already in the build through `reqwest` and `hyper-util`;
  - the new known-answer tests pass on both versions.
- **CI:** the Rust workspace tests run as a visible step, so an intermittent failure names its test
  (the validator's output tail had hidden it).
- **Tests:** known-answer vectors for the base64 engines used by persisted tokens, PKCE challenges,
  cursors and the encryption key. They guard dependency upgrades.
- **Dependencies:** GitHub Actions `actions/checkout`, `actions/setup-node` and
  `actions/upload-artifact` moved to v7, after reviewing their changelogs; CI is green on each.
- **Site:** canonical URL, Open Graph and Twitter card metadata, and a preview image. A manual
  GitHub Pages workflow (`pages.yml`; nothing deploys until a maintainer runs it).
- **Docs:**
  - the benchmark story at a glance (kept vs rejected) in `docs/WHAT_WE_REJECTED.md`;
  - the v1.1 MCP proposal (`docs/v1.1-mcp-proposal.md`; not implemented);
  - troubleshooting for a stale database volume after a failed first start;
  - a README pointer for benchmarking a change.

## 1.0.1 — 2026-10-05

First public release: open-source packaging of 1.0.0, plus fixes found by the first GitHub-hosted
CI runs, a clean-clone test and a public quickstart test. The architecture is unchanged. The
generated application code is unchanged, except for `./dev`, the E2E setup script and the smoke
tests (see *Fixed*). The 1.0.0 evidence (29/29 checks and the rest) was measured on the `v1.0.0`
tree; this release's CI is green on GitHub-hosted runners.

**Upgrading a project generated from 1.0.0:** copy `dev`, `frontend/scripts/e2e-prepare.mjs`,
`infra/docker/smoke.sh`, `infra/systemd/live-test.sh` and `.gitleaks.toml` from a freshly
generated 1.0.1 project, and add the license notices in `third_party_licenses/`.

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
- **Troubleshooting:** documents the colima shared-directory trap. A project outside `$HOME`
  gets an empty PostgreSQL config mount, and PostgreSQL refuses to start. Found by running the
  public quickstart from `/tmp`.
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
