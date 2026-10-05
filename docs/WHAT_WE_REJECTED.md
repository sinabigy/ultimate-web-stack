# What we intentionally don't use

Saying "no" with evidence is half of this project. Every entry links to its measurement. All
numbers come from one machine (Apple M5, with services in a 2-CPU / 4 GiB VM, median of 3), so
read them as relative evidence, not universal truth.
[benchmarks/SUMMARY.md](benchmarks/SUMMARY.md) has the full tables.

## Rejected
| candidate | what we measured | decision |
|---|---|---|
| **Monoio / io_uring runtime** | Linux 6.8 container, identical request handling: io_uring was 0–24% *slower* than epoll on the same runtime, and slower than Tokio at every point | stay on Tokio ([ADR 0010](../.ai/knowledge/DECISIONS/0010-stay-on-tokio-no-io-uring-runtime.md)) |
| **A reverse-proxy hop (Pingora or nginx) in the default path** | trivial endpoints: 143k → 66k req/s, +0.5 ms p50; DB-backed: 12–15% lower with Pingora, 7–8% with nginx; on trivial endpoints nginx at equal workers performed the same | Axum directly behind your TLS proxy; Pingora only for programmable edge logic ([ADR 0009](../.ai/knowledge/DECISIONS/0009-pingora-gateway-only-for-edge-needs.md)) |
| **Redis session cache** | the session lookup is 1 of 2–3 DB round trips, so the gain is bounded at about ⅓, at the cost of revocation latency or fail-open risk | sessions read from PostgreSQL; revocation stays immediate ([ADR 0004](../.ai/knowledge/DECISIONS/0004-no-redis-session-cache.md)) |
| **Fat LTO by default** | 14% smaller than thin LTO, 38% longer builds, no throughput gain | thin LTO, codegen-units 1 ([ADR 0011](../.ai/knowledge/DECISIONS/0011-release-profile-thin-lto-system-allocator.md)) |
| **A naive retrying HTTP client** | against a 500 req/s rate-limited provider it completed 9.3% of the work with 99.5% waste | outbound engine with separate rate and concurrency controllers ([ADR 0005](../.ai/knowledge/DECISIONS/0005-outbound-engine-controllers.md)) |

## Not adopted: no reliable win
| candidate | what we measured |
|---|---|
| **PGO** | Linux, interleaved A B A B: +7% plaintext, +9% json, +16% db, −3% cached, all inside the baseline's 19% round-to-round spread. It also costs a 241 s optimised build plus training |
| **mimalloc / jemalloc** | inside the baseline's own ±10% band in interleaved runs. An earlier "26–36% slower" result was environmental drift, which the interleaving exposed |
| **Thread-per-core Tokio** | +3–7% on raw loops, but it would need non-`Send` executors across axum and sqlx |
| **Dragonfly instead of Redis** | 54k vs 57k req/s at this scale; kept as a tested adapter, not preferred |

These are **inconclusive, not disproven**. They need a quiet, dedicated Linux host: run
`./dev benchmark` there, and share the result file with the blueprint project (its
CONTRIBUTING.md describes how).

## Optional, because they only win for some workloads
| module | wins when | doesn't when |
|---|---|---|
| NATS JetStream | job volume: 38k–49k vs 4.4k–5.1k jobs/s drain; cross-service consumers | enqueue must be transactional with business rows |
| ClickHouse | cross-tenant aggregates: 24 ms vs 573 ms over 2M events | per-tenant aggregates: PostgreSQL 5.5–6.4 ms is competitive |
| Redis | several API instances need a shared cache and rate limits | one instance: in-process is 2.2× faster (125k vs 57k req/s) |
| Pingora gateway | programmable edge logic on dedicated nodes | as a plain reverse proxy |
