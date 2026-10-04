# 0010: Stay on Tokio + axum; no Monoio / io_uring runtime

- Status: accepted
- Date: 2026-10-05

## Context
The extreme profile lists thread-per-core runtimes on io_uring (Monoio) as a possible
optimisation. The brief allows them only with benchmark evidence. Adopting one would mean leaving
the Tokio ecosystem: axum, hyper, sqlx, reqwest, tower, tracing integrations.

## Evidence
[docs/benchmarks/runtime.md](../../../docs/benchmarks/runtime.md), Linux 6.8 in a container, two
runs. Five variants share identical request handling:

- **Monoio on io_uring was 0–24% slower than Monoio on epoll**, and slower than Tokio at every
  point. For example, at 2 threads and concurrency 64: 235k vs 309k (Monoio epoll) vs 308k (Tokio)
  req/s.
- **Thread-per-core Tokio** (`SO_REUSEPORT`) was the fastest variant, 3–7% above work-stealing
  Tokio.
- **axum costs 15–20%** over a raw responder loop. That cost is larger than any runtime
  difference measured.

## Decision
- **Keep Tokio and axum.** Do not adopt Monoio, tokio-uring, compio or glommio for the HTTP path.
- **Keep the experiment in the repository** (`backend/experiments/runtime`, a separate workspace)
  so the question can be re-measured on target hardware with one command.
- **The thread-per-core gain (3–7%)** does not justify losing Tokio's work-stealing tolerance to
  uneven load (slow handlers, database waits) for this application. It is noted as an option for
  pure-CPU, uniform-request services.

## Alternatives considered
- **Monoio with io_uring**: measured slower here, and it requires ownership-based I/O APIs
  incompatible with the Tokio ecosystem. Rejected.
- **Tokio thread-per-core** (`LocalSet` per core with `SO_REUSEPORT`): measured fastest by a small
  margin. Not adopted, because axum and sqlx assume `Send` multi-thread executors, and uneven
  request costs favour work stealing.

## Consequences
- One runtime, one ecosystem, no `unsafe` or custom I/O layers in the core.
- Performance work goes where the measurements point: fewer database round trips per request
  (ADR 0004), connection reuse, and the outbound engine.

## Reversal conditions
Re-run `python3 benchmarks/runtime/run.py` on dedicated Linux hosts (≥8 cores, real NICs) with
the production request mix. Revisit this decision if one of the following holds:
- an io_uring runtime shows at least 20% more throughput, or lower p99, at equal CPU, *with the
  framework layer included*;
- a Tokio-compatible io_uring backend matures enough to keep the axum, hyper and sqlx ecosystem.
