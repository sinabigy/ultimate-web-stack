# Final acceptance: blueprint 1.0.0

**Release question:** from one tested command, can we generate a new secure application with:
- authentication, dashboards and authorization;
- PostgreSQL and the AI project protocol;
- only the infrastructure its profile requires?

And can another AI enter that repository later and continue engineering correctly, without the
original conversation?

**Answer: yes**, with the evidence and boundaries below. Each answer names how it was proven
(test, live run or document) and what was *not* proven.

## Environment
| | |
|---|---|
| host | Apple M5 (10 cores, 16 GB), macOS 26.5.1 |
| containers | colima 0.10.3 (aarch64 VM, 2 CPU / 4 GiB), Docker 29.5.2 |
| toolchains | Rust 1.99.0 (pinned), Node 26.0.0, Python 3.14.5 |
| protocol | ai-project-template / AI protocol 1.2.0 |
| date | 2026-10-05 |
| release | tag `v1.0.0` (its commit is the release; `./dev check` 29/29 on that tree) |

Linux behaviour (systemd, release image, PGO and io_uring experiments) was exercised in
containers on that VM. CI on GitHub-hosted Linux runners has **not** run, because nothing has
been pushed.

## Can a new project be generated?
**Yes.** `scripts/create-project` generated three representative profiles from the final commit,
and `scripts/validate-generated` passed each end to end (section [Generated projects](#generated-projects)).

Generator acceptance (`scripts/tests/test_create_project.py`, 5 tests, part of `./dev check`):
- the profile defaults;
- module combinations (core, performance, full with Cedar and gateway);
- invalid combinations refused;
- a non-empty destination refused, and a destination inside the blueprint refused;
- `--help`;
- non-interactive use without a name gives a clear error and creates nothing;
- an injected failure after copying removes the partial destination;
- names with spaces and hyphens (`"My Cool-App 2"` → slug `my-cool-app-2` in compose, package
  and brand);
- **no source-machine paths** (the blueprint's absolute path, the home directory, or the
  checkout's parent folder) anywhere in the generated tree.

## Can it run without the blueprint repository?
**Yes.** Generated projects are written outside the blueprint, with:
- their own git history (one initial commit);
- their own compose project name, `target/` and `node_modules`;
- no blueprint-only tools (generator, validator and runtime experiment are excluded);
- documentation links to those tools removed.

Every validation step runs inside the generated directory only. The path-leak test above
guarantees no file refers back to the blueprint or the template checkout. Schema `$id`s use the
reserved `ai-project-template.invalid` domain: they are identifiers, not locations.

## Can an AI understand it from repository-local information?
**Yes, demonstrated twice.** Agents were given only a generated repository and a product
request:

| run | project | feature | result |
|---|---|---|---|
| 1 | `--profile performance --auth b2b --cedar` | organization-scoped Projects (CRUD, permissions, audit, UI) | `./dev check` 23/23; member 403, cross-tenant 404 live; about 45 min; no look outside the repo |
| 2 | default (`core`, `b2b`), from the release candidate | organization Announcements with notifications to members | `./dev check` 22/22 (including release smoke and live systemd); live member writes 403 (audited), every cross-tenant probe 404 (by slug and by id), notifications to exactly the other members; scorecard "yes" on all 11 capabilities; never looked outside the repo |

Run 1's friction log led to these blueprint fixes:
- the logout CSRF race;
- E2E port clash with a running stack;
- one documented authorization path;
- the "add a permission" guide;
- `CONVENTIONS.md`;
- stale constraints and architecture entries;
- an inferred placeholder command.

Run 2 reported six reusable gaps, all fixed (scorecard and list in
[FINAL_REPORT.md](FINAL_REPORT.md#clean-room-findings)).

## Does authentication work?
**Yes, against the mock IdP everywhere and against self-hosted ZITADEL in the blueprint.**
- Rust: `auth_flow.rs`, `account.rs` and `credentials.rs`. These cover PKCE, nonce, state, cookie
  attributes, rotation, revocation, MFA step-up, API keys, service JWTs and per-principal rate
  limits.
- Browser acceptance (`./dev test --journey`, 6/6 against the full stack):
  - sign in;
  - a POST without the CSRF token is refused (403);
  - reload keeps the session;
  - **logout regression**: three cycles of `/logout` opened directly, then the session is
    anonymous, `/dashboard` redirects to login, and sign-in works again.
- Failure drill: a tampered session cookie gives 401; a missing CSRF token gives 403
  `csrf_failed`.
- **Not verified live:** ZITADEL Cloud and other OIDC providers (configuration and discovery
  check only).

## Does authorization work?
**Yes, under both engines, over HTTP.** `authorization_matrix.rs` runs the full matrix under RBAC
and under Cedar:

| actor → target | expected | result |
|---|---|---|
| anonymous → protected (4 kinds) and a write | 401 | ✓ |
| member → own organization's resource | allowed | ✓ |
| member → organization admin action (update, empty update, invite, API key) | 403, each audited | ✓ |
| tenant A → tenant B resource (by slug, by id, delete by id) | 404, audited | ✓ |
| org admin → own organization administration | allowed | ✓ |
| org admin → invite above own role (escalation) | 403, audited | ✓ |
| org admin / owner → system admin action | 403, audited | ✓ |
| system auditor → 8 console reads / 3 mutations | 200 / 403 | ✓ |
| system admin (MFA) → system operation | allowed | ✓ |
| Cedar project policy forbidding a read | 403 over HTTP (mutation-checked) | ✓ |

Every org-scoped handler goes through the documented engine path (`require`, `require_read` or
`deny`, see `docs/authorization/README.md`). A scan of all route handlers found three cases,
all fixed with regression rows:
- the org GET did not go through the engine;
- four structural checks were not audited;
- an empty update ran without any decision.

The browser acceptance repeats the boundaries in the UI *and* at the API. UI hiding is never
counted as evidence.

## Is tenant isolation proven?
**Yes.**
- **Repositories:** they accept only an `OrgAccess` proof (`tenant_isolation_at_repository_layer`,
  `custom_roles_and_teams_cannot_cross_tenants`).
- **HTTP:** the authorization matrix (cross-tenant 404 by slug and by id).
- **Realtime:** `realtime_subscriber_only_gets_own_tenant`.
- **Analytics:** `queries_are_tenant_scoped`.
- **Under load:** the benchmark invariant asserts every cross-tenant probe returns 404.
- **Browser:** an outsider sees "Not found" for another tenant's organization, and the API
  returns 404 for its organization, runs and members.
- **Live, in the clean-room projects:** new features built by agents kept the pattern; cross-tenant
  requests returned 404.

## Does the dashboard work?
**Yes.** Browser acceptance screenshots every area per role in
`frontend/test-results/journey/` (27 images):
- user dashboard and account;
- organization overview, members, settings and audit;
- the cross-tenant not-found page;
- the auditor console;
- system admin overview, users, organizations, jobs, audit, providers and health.

The E2E suite also checks:
- all 28 routes render;
- axe accessibility (WCAG 2.2 AA tags) in light and dark themes;
- no CSP violations;
- keyboard operability.

Cosmetic gaps are recorded in `.ai/knowledge/KNOWN_ISSUES.md`.

## Do optional infrastructure profiles work?
**Yes.** For each generated profile, `validate-generated` checks:
- selected modules: the service runs, the client library is compiled in, the config is enabled,
  the system smoke passes, and the server's `configuration` log line matches;
- unselected modules: no service, not compiled, config disabled, no validation command, no CI
  service, smoke SKIP.

Failure drill against the full stack:
- NATS or ClickHouse down: core works, readiness `degraded`, recovery is automatic;
- PostgreSQL down: 503, and the session survives the outage.

Details: [operations/failure-modes.md](operations/failure-modes.md).

## Can it build for production?
**Yes.** The blueprint (`release-smoke`, in every `./dev check`) and each generated project:
- the release frontend build;
- release Rust binaries (thin LTO);
- the distroless image (no baked secrets, verified on image config and filesystem);
- migrations on start;
- non-root and read-only root filesystem;
- health checks for API and worker;
- status-only public `/readyz`, `/metrics` 404 on the public port;
- `docker restart` comes back healthy;
- SIGTERM drains and exits 0.

## Can it be deployed?
| tier | proven | boundary |
|---|---|---|
| containers (production compose) | **live**, release smoke | single host; TLS proxy not exercised |
| VPS (systemd + Caddy) | **live**, `systemd-live`: the release binaries installed per `docs/deployment/vps.md` under the hardened units in a Debian 12 systemd container with PostgreSQL. Covers start, `check-config`, migrations, unprivileged user, worker, restart, graceful stop, SIGKILL → automatic restart. Caddyfile: `caddy validate` | not run on a real VM; TLS issuance not exercised |
| Kubernetes | **static**: kubeconform `-strict` on base and rendered kustomization; every `APP__` variable in manifests is a real config key (test) | **no live cluster was used**; do not assume runtime behaviour |

The live systemd test found a real bug: the worker unit and the Kubernetes worker manifest set
`APP__WORKER__PORT`, which the configuration loader rejected, so the worker crash-looped. Static
validation could not see it. It is fixed (`worker.port`) and guarded by
`every_app_variable_in_deployment_files_is_a_config_key`.

## Are benchmarks reproducible?
**Yes, on the same machine, within measured noise.**
- `./dev benchmark` runs the suites, the gates and the report.
- Each result in `benchmarks/results/*.json` records the machine fingerprint, the git commit and
  the dirty flag.
- Method: median of 3; run-to-run noise measured (≤ 7.6% between full runs).
- Regression gates compare only results from the same machine; invariant gates run everywhere.
- A/B decisions near the noise (allocators, PGO) used interleaved runs.
- Linux-only experiments have scripts: `benchmarks/runtime/run.py`,
  `benchmarks/release/pgo_linux.py`.
- Final harness check: `./dev benchmark --smoke` on `9efafc8` passed all 17 invariant gates. These cover zero errors, 401 for every missing credential (382,179 requests), 404 for every cross-tenant probe (10,429), engine waste and outage amplification bounds, every queued job processed (PostgreSQL and JetStream, 2,000 each), and no analytics events lost (200,000). Regression gates are skipped in smoke mode, by design.
- **Not reproducible across machines by design**: results from other hardware are not
  comparable and the gates say so.

## Known limitations
- **Live verification gaps:**
  - Kubernetes was validated statically only;
  - ZITADEL Cloud and generic OIDC providers are configuration-only;
  - CI has not run on GitHub;
  - all measurements come from one machine, with services in a small VM (HTTP numbers are lower
    bounds).
- **Inconclusive optimizations:** PGO, alternative allocators and thread-per-core Tokio need
  dedicated Linux hardware to decide.
- **Intermittent failures** on this host:
  - `sqlx::test` connect failures through the colima port forwarder (passes on rerun);
  - one historical E2E flake under full load.
- **Cosmetic:** the empty "Job queues" chart, and the organization switcher on not-found pages.
- **Delegated to the IdP:** email delivery, password policy and MFA enrolment. The mock IdP
  simulates them.
- **Not covered by the failure drill:** network partitions between API instances, and disk-full on
  the database host.

## Generated projects
Generated from release candidate `60f7e91` and validated with `scripts/validate-generated`.
Regenerating from the final commit `9efafc8` differs only in provenance stamps, the `./dev
benchmark` service start, and one documentation sentence (file-by-file comparison). Every
project's `./dev check` includes:
- the release image smoke;
- the live systemd test;
- E2E;
- supply-chain and secret scans.

| project | flags | result | `./dev check` | live system smoke | optional modules (service + compiled + config) |
|---|---|---|---|---|---|
| minimal | `--profile core --auth basic --no-admin` | **OK** | OK — 22 pass, 0 fail, 0 skip (6 min) | OK — 10 pass, 0 fail, 3 skip | Redis, NATS, ClickHouse all **absent** ✓ |
| saas | `--profile performance --auth b2b --admin --cedar` | **OK** | OK — 24 pass, 0 fail, 0 skip (8 min) | OK — 10 pass, 0 fail, 3 skip | Redis **present** ✓; NATS, ClickHouse **absent** ✓; Cedar active ✓ |
| throughput | `--profile distributed --auth b2b` | **OK** | OK — 25 pass, 0 fail, 0 skip (22 min) | OK — 12 pass, 0 fail, 1 skip (Tempo: observability not selected) | Redis, NATS, ClickHouse all **present** ✓ |

The "running server configuration" step was first evaluated with a validator log-parser bug, which
could not read the pretty log format. It was re-evaluated from each project's saved server log
with the fixed parser, and all three match. Details per step:
[acceptance/generated-projects.md](acceptance/generated-projects.md).
