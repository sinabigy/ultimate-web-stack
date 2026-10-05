<div align="center">

# Ultimate Web Stack

**The web stack we tried to disprove.**

An evidence-driven, production-tested, AI-ready foundation for serious web applications,
generated with one command.

Rust · SolidJS · PostgreSQL · OIDC · RBAC/Cedar · Jobs · Realtime · Observability · Docker · systemd · Kubernetes · AI project protocol

[![CI](https://github.com/sinabigy/ultimate-web-stack/actions/workflows/ci.yml/badge.svg)](https://github.com/sinabigy/ultimate-web-stack/actions/workflows/ci.yml)
![release](https://img.shields.io/badge/release-v1.0.0-blue)
![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-green)

| 29/29 | 294 | 14/14 | 17/17 | 2/2 |
|:---:|:---:|:---:|:---:|:---:|
| release checks | Rust tests | failure drills | benchmark gates | clean-room AI agents built complete features |

<sub>Release evidence for v1.0.0, measured on one machine; see <a href="docs/FINAL_ACCEPTANCE.md">FINAL_ACCEPTANCE.md</a>, including what is <b>not</b> yet proven.</sub>

</div>

---

## Quickstart

```sh
git clone https://github.com/sinabigy/ultimate-web-stack && cd ultimate-web-stack
scripts/create-project ../my-product --name "My Product"     # one command → an independent repository
cd ../my-product
./dev setup && ./dev up                                       # → http://localhost:5190 (mock sign-in: any email)
./dev check                                                   # every validation command
```

You need Rust (rustup), Node ≥ 22, Python ≥ 3.11 and Docker (≥ 6 GiB for the VM). The full
walkthrough is in [docs/QUICKSTART.md](docs/QUICKSTART.md); the admin console login is described
there too.

## What you get, on day one

| | |
|---|---|
| 🔐 **Authentication** | OIDC backend-for-frontend: HttpOnly session cookies (no tokens in the browser), CSRF, MFA step-up, sessions list and revocation, account deletion |
| 🏢 **Multi-tenancy** | organizations, invitations, teams, custom roles; cross-tenant access answers **404**; repositories accept only an `OrgAccess` proof |
| 🛡️ **Authorization** | RBAC or **Cedar** policies, default deny, one engine path for every read and write, and a separate MFA-gated system admin / auditor level |
| 📜 **Audit** | append-only trail (database triggers); every denied write is recorded |
| 🖥️ **Dashboards** | user, organization and system-admin areas (SolidJS), with a command palette, light and dark themes, and axe accessibility checks |
| ⚙️ **Platform** | PostgreSQL job queue with leases and dead letters, realtime (SSE/WebSocket), an outbound API engine that learns provider rate limits, W3C tracing, metrics on an internal ops port |
| 🚀 **Deployment** | distroless image + compose, hardened systemd units + Caddy, Kubernetes base manifests |
| 🤖 **AI-ready** | `AI_PROTOCOL.md`, task state, conventions and ADRs: a coding agent can continue the project from the repository alone |

Add only what your workload needs, with **profiles**:

| profile | adds | measured reason |
|---|---|---|
| `core` (default) | nothing: PostgreSQL alone | runs sessions, jobs, realtime and audit on one database |
| `performance` | Redis cache and shared rate limits | needed across instances; on one instance, in-process is 2.2× faster |
| `distributed` | + NATS JetStream, ClickHouse | 38k–49k vs 4.4k–5.1k jobs/s; cross-tenant aggregates 24 ms vs 573 ms |
| `full` | + Prometheus, Tempo, Loki, Grafana | observability development |

An unselected module is **absent**: not compiled, not started, not configured, not in CI.
Validation proves this for every generated project. Switch modules later with
[docs/MODULES.md](docs/MODULES.md).

## Why this is different

### 1. Every default was measured, and many candidates lost
We benchmarked the alternatives on one harness with recorded noise (median of 3; ≤ 7.6%
run-to-run) and regression gates, then kept only what earned its place.

| area | default | profile-specific | not adopted (no reliable win) | rejected |
|---|---|---|---|---|
| runtime | Tokio + axum | – | thread-per-core Tokio | **Monoio / io_uring** (0–24% slower than epoll) |
| edge | Axum behind your TLS proxy | Pingora for edge logic | – | **an extra proxy hop** (143k → 66k req/s) |
| jobs | PostgreSQL queue | NATS JetStream | – | – |
| cache | in-process (125k req/s) | Redis (57k) | Dragonfly (54k) | **Redis session cache** |
| outbound | rate + concurrency controllers | – | – | **naive retrying client** (9.3% completed, 99.5% waste) |
| build | thin LTO, system allocator | – | PGO, mimalloc, jemalloc | **fat LTO by default** (−14% size, +38% build, no speed) |

The full results are in [docs/WHAT_WE_REJECTED.md](docs/WHAT_WE_REJECTED.md) and
[docs/benchmarks/SUMMARY.md](docs/benchmarks/SUMMARY.md). Every decision is an ADR with
evidence, alternatives and **reversal conditions** (`.ai/knowledge/DECISIONS/`).
<sub>Measured on an Apple M5, with services in a 2-CPU / 4 GiB VM. Treat the numbers as relative
evidence and re-run `./dev benchmark` on your hardware.</sub>

### 2. We attacked it on purpose
- **Authorization matrix over HTTP, under both RBAC and Cedar:** anonymous, member, cross-tenant,
  org admin, system auditor and system admin, with audit counts.
- **Browser acceptance per role** (6/6), plus a logout regression check.
- **Live failure drill** (14/14):
  - PostgreSQL, NATS and ClickHouse stopped, giving 503 or `degraded`, never 500 or a hang;
  - provider 429s, 5xx errors, timeouts and outages;
  - forged sessions and missing CSRF tokens.
- **Live deployment tests:**
  - the release image: non-root, read-only, no baked secrets, restart, graceful SIGTERM;
  - real systemd units, including crash restart.

These tests found and fixed real bugs, among them:
- an empty update that skipped authorization;
- unaudited escalation attempts;
- a logout race;
- a worker crash-loop that static manifest validation could not see.

Each now has a regression test that was proven to fail without the fix.
([FINAL_REPORT.md](docs/FINAL_REPORT.md#security-findings-fixed))

### 3. An AI agent inherited it, twice
Two coding agents were given **only a generated repository** and a product request. Each built
a complete, secure, organization-scoped feature: Projects, then Announcements with notifications.
Both passed the full validation (23/23 and 22/22), with member denials and cross-tenant isolation
shown live, and neither read anything outside the repository. Every reusable gap they hit was
fixed in the blueprint. → [docs/AI_AGENT_GUIDE.md](docs/AI_AGENT_GUIDE.md)

### 4. Generated projects are truly independent
`create-project` writes a new git repository with:
- its own compose project and its own CI, which starts only the selected services;
- a fresh AI-protocol instance;
- **no references back** to this repository (tested).

Three reference profiles are validated end to end on every release.

## Architecture

```text
 Browser ── SolidJS SPA ──(cookie session + CSRF)──▶ Rust API (axum) ──▶ PostgreSQL 18
                                                     │  │                 sessions · jobs · events · audit
                    OIDC provider (BFF login) ◀──────┘  ├──▶ Worker ──▶ outbound engine ──▶ external APIs
                                                        │
          optional: Redis (cache) · NATS/JetStream (events, queue) · ClickHouse (analytics) · Pingora (edge)
          ops port (internal): /metrics · detailed /readyz          traces: W3C → OTLP
```

Read more:
- [component map](.ai/knowledge/ARCHITECTURE.md);
- [authentication](docs/authentication/README.md), [authorization](docs/authorization/README.md)
  and [multitenancy](docs/multitenancy/README.md);
- [threat model](docs/security/threat-model.md);
- [failure modes](docs/operations/failure-modes.md);
- [outbound engine](docs/architecture/outbound-engine.md).

## Deployment

| tier | artefacts | verified |
|---|---|---|
| VPS | release binaries, hardened systemd units, Caddy | **live**, systemd in a container: start, migrations, non-root, restart, graceful stop, crash restart |
| Containers | distroless image, production compose | **live**: health, public surface, no secrets in the image, restart, graceful shutdown |
| Kubernetes | kustomize base (HPA, PDB, NetworkPolicy, probes) | **static only** (kubeconform `-strict`) |
| Edge | Pingora gateway (optional) | compile, lint and benchmark |

Before going live: [docs/PRODUCTION_CHECKLIST.md](docs/PRODUCTION_CHECKLIST.md).

## What is not yet proven

We publish this as prominently as the results:
- **Kubernetes** is validated statically only, never on a live cluster.
- **Identity providers:** ZITADEL Cloud and other OIDC providers are configuration-only. Self-hosted
  ZITADEL was verified live.
- **GitHub-hosted CI** has not run yet. Every result above comes from local runs.
- **One machine:** the benchmarks come from one Apple M5, with services in a small VM, so HTTP
  numbers are lower bounds.
- **Undecided:** PGO, alternative allocators and thread-per-core Tokio need dedicated Linux
  hardware.

The full list is in [FINAL_ACCEPTANCE.md → Known limitations](docs/FINAL_ACCEPTANCE.md#known-limitations).

## Documentation

| | |
|---|---|
| Start | [quickstart](docs/QUICKSTART.md) · [why this exists](docs/WHY.md) · [FAQ](docs/FAQ.md) · [troubleshooting](docs/TROUBLESHOOTING.md) |
| Build | [adding a feature](docs/examples/adding-a-feature.md) · [modules](docs/MODULES.md) · [capability matrix](docs/BLUEPRINT_CAPABILITY_MATRIX.md) · [conventions](.ai/knowledge/CONVENTIONS.md) |
| Evidence | [final report](docs/FINAL_REPORT.md) · [acceptance](docs/FINAL_ACCEPTANCE.md) · [benchmarks](docs/benchmarks/SUMMARY.md) · [ADRs](.ai/knowledge/DECISIONS/) |
| Project | [roadmap](ROADMAP.md) · [changelog](CHANGELOG.md) · [versioning](docs/VERSIONING.md) · [governance](GOVERNANCE.md) · [all docs](docs/README.md) |

Working on this repository yourself, as a human or an AI agent? Start with
[AI_PROTOCOL.md](AI_PROTOCOL.md), then `python3 tools/ai-check`.

## Contributing

Bug fixes, docs, examples and **benchmark results from your hardware** are very welcome. Defaults
change on evidence: see [CONTRIBUTING.md](CONTRIBUTING.md).
- Questions go to Discussions ([SUPPORT.md](SUPPORT.md)).
- Vulnerabilities are reported privately ([SECURITY.md](SECURITY.md)).
- Look for `good first issue` and `help wanted`.

## Support the project

⭐ **If this project saved you time, star it: it helps other developers discover it.**

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Applications you generate are yours to license as you choose (keep the blueprint's notices,
which the generator places in `third_party_licenses/`).
