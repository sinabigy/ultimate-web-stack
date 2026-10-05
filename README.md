# ultimate-web-stack-blueprint

A reusable, evidence-driven blueprint for secure, fast web products: a **SolidJS + TypeScript +
Vite** frontend, a **Rust (Axum, Tokio, SQLx)** backend, and **PostgreSQL**. Profiles add
Redis/Dragonfly, NATS/JetStream, ClickHouse and observability, and a Pingora edge is optional.
Every technology choice is backed by a measurement and an ADR. Those that measurements did not
support were rejected, and that is recorded too.

`scripts/create-project` generates a new, independent product repository from the blueprint. It
comes with:
- authentication (BFF sessions over OIDC);
- organizations, RBAC (or Cedar) and admin, configurable;
- user, organization and admin dashboards, and an audit trail;
- background jobs, realtime and an outbound API engine;
- tests at every level, benchmarks with regression gates, and four deployment tiers;
- the AI working protocol, so a future agent can continue the project without reconstructing
  past decisions.

## Generate a product

```sh
scripts/create-project ../acme --name "Acme" --profile performance --auth b2b --cedar
cd ../acme && ./dev setup && ./dev up      # http://localhost:5190 (mock identity provider: any email)
./dev check                                 # every validation command
./dev test --system                         # one live journey through every component
```

Profiles and what they contain: [docs/BLUEPRINT_CAPABILITY_MATRIX.md](docs/BLUEPRINT_CAPABILITY_MATRIX.md).
Run `scripts/create-project --help` for all flags, or omit `--name` on a terminal to be prompted.

## Work on the blueprint itself

```sh
./dev setup                      # toolchains, dependencies, git hooks
./dev up                         # PostgreSQL + enabled modules, API, worker, SPA (mock IdP)
./dev up --identity zitadel      # the same with self-hosted ZITADEL
./dev up --with observability    # + Prometheus, Tempo, Loki, Grafana (http://localhost:53000)
./dev check                      # tools/ai-validate: lint, tests, E2E, security, infra, release image
./dev test --system              # live system smoke against the running stack
./dev benchmark                  # benchmark suites + regression gates + report
```

Prerequisites: Rust (pinned in `backend/rust-toolchain.toml`), Node ≥ 22, Docker (colima
works: give it at least 6 GiB), Python 3.11+, and `oha` for benchmarks.

## What is where

| | |
|---|---|
| Architecture | `.ai/knowledge/ARCHITECTURE.md` (component map), [docs/architecture/](docs/architecture/) (outbound engine, messaging, analytics) |
| Identity and access | [authentication](docs/authentication/README.md), [authorization](docs/authorization/README.md), [multitenancy](docs/multitenancy/README.md), [admin](docs/admin/README.md) |
| Operations and deployment | [observability](docs/operations/observability.md), [deployment tiers](docs/deployment/README.md), [threat model](docs/security/threat-model.md) |
| Evidence | [benchmark summary](docs/benchmarks/SUMMARY.md) (kept / rejected / inconclusive / optional), [latest results](docs/benchmarks/latest.md), `benchmarks/results/*.json` |
| Decisions | `.ai/knowledge/DECISIONS/` (ADRs with evidence, alternatives and reversal conditions) |
| Constraints | `.ai/knowledge/CONSTRAINTS.md` (including the 10 security invariants) |
| Known issues | `.ai/knowledge/KNOWN_ISSUES.md` |
| Documentation index | [docs/README.md](docs/README.md) |

## Scaling path

1. **One host**: VPS with systemd and Caddy, or containers with the production compose file.
   PostgreSQL holds sessions, jobs and realtime fan-out, so there is no extra infrastructure.
2. **Several API instances**: enable the `cache` module (shared Redis cache and rate limits). The
   PostgreSQL event bus already reaches every instance.
3. **High job volume or cross-service consumers**: `messaging_nats` (about 10× the PostgreSQL
   queue, measured).
4. **Analytics across tenants**: `analytics_clickhouse` (about 24× faster cross-tenant aggregates,
   measured).
5. **Many replicas**: the Kubernetes base (HPA, PDB, probes on the internal ops port).
6. **Programmable edge**: the Pingora gateway on dedicated nodes (not for raw speed; it measured
   equal to nginx).

## Working on this repository (humans and AI agents)

This project uses a portable, model-independent working protocol:

1. `AI_PROTOCOL.md`: how work is done here (short).
2. `.ai/config/project.json`: objective, stack, architecture metadata and validation commands.
3. `.ai/state/CURRENT_TASK.json`: what is being worked on now.

Check the repository state with `python3 tools/ai-check`. Protocol details are in `docs/protocol/`.
