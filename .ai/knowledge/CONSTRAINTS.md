# Constraints

_Hard constraints: platform, compliance, performance, compatibility and security. Each entry gives the constraint, its source (a user, a contract or a law), and how it is verified._

| Constraint | Source | Verified by |
|---|---|---|
| Permanent core: SolidJS + TypeScript frontend; Rust (Axum, Tokio, Serde, Hyper/Reqwest) backend; PostgreSQL via SQLx. Everything else is an optional module. | user (mission brief 2026-10-04) | `project.json → architecture.components`; ADRs 0003–0008 |
| Optional infrastructure (Redis/Dragonfly, NATS/JetStream, ClickHouse, Pingora, specialised runtimes, ScyllaDB, Kubernetes) activates only by profile and only when evidence justifies it. A core project must run with PostgreSQL alone. | user | `./dev up` for the core profile starts only PostgreSQL; core-profile tests |
| Modular monolith first; services are extractable later, never split for their own sake. | user | crate boundaries in `backend/crates/`; ADR 0009 |
| No performance claim without our own measurement. Never fabricate benchmark results or service connections. External benchmarks (TechEmpower) are context, not authority. | user | `benchmarks/results/*.json` carry environment + git provenance; `docs/benchmarks/` generated from them |
| Optimise successful useful work per dollar per second, not raw requests/second. | user | benchmark reports include success rps, error rates and per-core / per-GB figures |
| PostgreSQL is the transactional source of truth; caches and ClickHouse never become canonical. | user | cache/analytics code paths are read-through or append-only; ADR 0010/0012 |
| Never commit secrets; only `.env.example`. | user + AI protocol | `ai-check` secret patterns; `./dev check` secret scan |
| The AI operating protocol is `~/Coding/ai-project-template`; do not create a competing state system. Generated projects are initialised with its `ai-init --target`. | user | generator tests assert `ai-check` passes in generated projects |
| No Node.js in production unless SSR requirements justify it. | user | production artefacts: Rust binary + static files; ADR 0006 |
| Kubernetes is never the default deployment. | user | deployment docs; ADR 0016 |
| Do not push to remotes unless authorised. | user | — |
