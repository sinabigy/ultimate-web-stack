# Benchmark summary: what the measurements decided

Every number below comes from a committed result file in `benchmarks/results/`, measured on an
Apple M5 (10 cores, 16 GB) with services in a 2-CPU / 4 GiB container VM. Linux-only experiments
ran in containers on that VM. The method is described in [`benchmarks/README.md`](../../benchmarks/README.md):
- median of 3 runs per point;
- run-to-run noise measured: ≤7.6% throughput between full runs, up to 23% for single samples;
- results from different machines are never compared.

Negative and inconclusive results are listed on purpose: they are why some technologies are
*not* in the default stack.

## Kept because measurements support it

| decision | evidence | ADR |
|---|---|---|
| **Outbound engine with separate rate and concurrency controllers** | 500 req/s rate-limited provider: before the fixes 163 useful req/s; after, a steady 496–500 req/s (99–100% of the limit), 100% of work completed, 1.9% waste. The naive client completed 9.3% with 99.5% waste. Overloaded provider: 1,561 req/s at about 1,600 capacity, 0.2% waste (naive client: 22% completed, 98.8% waste). Outage: 0.48× amplification vs 21×. | [0005](../../.ai/knowledge/DECISIONS/0005-outbound-engine-controllers.md) |
| **Keep-alive connection pooling** | 99.4% connection reuse; pooled 35k vs fresh 26k req/s | [0005](../../.ai/knowledge/DECISIONS/0005-outbound-engine-controllers.md) |
| **Sessions read from PostgreSQL (no Redis session cache)** | throughput tracks DB round trips (5.7k req/s at 2 trips, 3.6k at 3); the session lookup is 1 of 2–3, while revocation stays immediate | [0004](../../.ai/knowledge/DECISIONS/0004-no-redis-session-cache.md) |
| **Tokio + axum** | thread-per-core Tokio was the fastest runtime variant; axum costs 15–20% over raw loops, more than any runtime difference measured | [0010](../../.ai/knowledge/DECISIONS/0010-stay-on-tokio-no-io-uring-runtime.md) |
| **Thin LTO, codegen-units 1** | throughput within noise vs no LTO and fat LTO; 23.7 MB vs 32.2 MB (no LTO) for +18% build time | [0011](../../.ai/knowledge/DECISIONS/0011-release-profile-thin-lto-system-allocator.md) |
| **W3C trace propagation on by default** | +1.46 µs CPU per request (micro-benchmark); invisible on DB-backed endpoints, 7–10% only on no-op endpoints at concurrency 256 | [0006](../../.ai/knowledge/DECISIONS/0006-trace-propagation-default-on.md) |
| **PostgreSQL job queue as default** | 4.4k–5.1k jobs/s drain with transactional enqueue: ample for the core profile | [0007](../../.ai/knowledge/DECISIONS/0007-postgres-queue-default-jetstream-for-scale.md) |
| **In-process cache as default** | 125k req/s cached reads vs 57k via Redis (network hop) on one instance | [benchmarks](latest.md) |

## Rejected because measurements did not support it

| candidate | evidence | ADR |
|---|---|---|
| **Monoio / io_uring runtime** | Linux 6.8 container, identical request handling: io_uring was 0–24% *slower* than epoll on the same runtime, and slower than Tokio at every point | [0010](../../.ai/knowledge/DECISIONS/0010-stay-on-tokio-no-io-uring-runtime.md) |
| **Pingora in the default request path** | an extra hop halves trivial-endpoint throughput on a shared host (143k → 66k req/s, +0.5 ms p50) and cuts DB-backed throughput 12–14%; nginx at equal workers performed the same | [0009](../../.ai/knowledge/DECISIONS/0009-pingora-gateway-only-for-edge-needs.md) |
| **Redis session cache** | the gain is bounded at about ⅓ of DB round trips on typical endpoints, at the cost of revocation latency or fail-open risk | [0004](../../.ai/knowledge/DECISIONS/0004-no-redis-session-cache.md) |
| **Fat LTO by default** | 14% smaller than thin LTO for 38% more build time; no throughput gain | [0011](../../.ai/knowledge/DECISIONS/0011-release-profile-thin-lto-system-allocator.md) |

## Inconclusive (not adopted; re-measure on dedicated hardware)

| candidate | evidence |
|---|---|
| **PGO** | Linux, interleaved A B A B: +7% plaintext, +9% json, +16% db, −3% cached, all within the baseline's 19% round-to-round spread. On macOS, raw profiles of the full dependency graph were unreadable by `llvm-profdata` (a minimal crate worked). Adds a 241 s optimised build plus training. ([release-profile](release-profile.md)) |
| **mimalloc / jemalloc** | interleaved runs fall inside the baseline's own ±10% band. An earlier sequential run that looked 26–36% *slower* was environmental drift, shown by the interleaving. Peak RSS varied 30–109 MB for the same binary. ([release-profile](release-profile.md)) |
| **Thread-per-core Tokio for the app** | +3–7% on raw loops, but it would require non-`Send` executors across axum and sqlx ([runtime](runtime.md)) |

## Optional because workload-dependent

| module | when it wins (measured) | when it does not |
|---|---|---|
| **NATS JetStream** | about 10× the PostgreSQL queue (35k–49k vs 4.4k–5.1k jobs/s drain); cross-service consumers; replay | when enqueue must be transactional with business rows (that needs an outbox) |
| **ClickHouse** | cross-tenant aggregates about 24× faster (24 ms vs 573 ms over 2M events); 7.7× smaller storage | per-tenant aggregates: PostgreSQL 5.5–6.4 ms vs ClickHouse 3.2–3.8 ms, which is competitive |
| **Redis / Dragonfly** | shared cache and rate limits across instances | single instance: in-process is 2.2× faster. Dragonfly measured slightly below Redis (54k vs 57k) at this scale |
| **Pingora gateway** | programmable edge logic on dedicated nodes | as a plain reverse proxy: it is equivalent to nginx and costs a hop |

## Findings the benchmarks produced (bugs fixed because of them)

Each of these was found by measuring, not by reading code:
- seven outbound-controller bugs;
- breaker dilution;
- latency-jitter overreaction;
- ops endpoints being rate-limited and load-shed;
- rate limiting keyed by IP instead of principal;
- heartbeat leases surviving cancelled handlers;
- the release-image build OOM-killing a dev service.
