# 0008: ClickHouse for event analytics; PostgreSQL stays the system of record

- Status: accepted
- Date: 2026-10-05

## Context
Product analytics include events per tenant per day, usage metering and cross-tenant platform
reports. These grow without bound and are read as aggregates over time. PostgreSQL can store and
query events. The question is where the crossover lies, and what a second datastore costs.

## Evidence
Measured with `app-bench analytics` (`benchmarks/run.py --suite analytics`):
- 2,000,000 events, 100 organisations, 60 days;
- random timestamps and values, and a 32-hex request id per event, so compression is not
  flattered by regular synthetic data;
- PostgreSQL 18 with an index on `(organization_id, event, ts)`, against ClickHouse 26.3 with the
  shipped schema;
- both in the same 2-CPU VM; two ad-hoc runs, ranges shown, plus the recorded harness run
  `benchmarks/results/20261004T135507Z-3b3ae3626ec1-analytics.json` (ClickHouse 722k rows/s,
  3.3 ms tenant, 24.4 ms all tenants, 45.7 MB; PostgreSQL 726k rows/s, 6.4 ms, 583 ms, 352 MB).

| | ClickHouse | PostgreSQL |
|---|---|---|
| ingest | 720k–773k rows/s through the application sink (batched `INSERT`, no loss) | 642k–746k rows/s set-based `INSERT … SELECT` (best case; application row inserts are far slower) |
| one tenant, 30-day daily aggregate (p50) | 3.2–3.8 ms raw; 2.6 ms from the rollup | 5.5–6.4 ms |
| all tenants, daily × event aggregate (p50) | 23.5 ms | 564–573 ms (**~24× slower**) |
| storage (events table + indexes) | 45.7 MB | 352 MB (**~7.7× larger**) |

Correctness, from `backend/crates/analytics/tests/clickhouse.rs` and `backend/crates/api/tests/analytics.rs`:
- **Schema**: monthly partitions, TTL and the rollup exist; migrations are idempotent.
- **Ingestion**: 30k events are recorded in under a second without blocking; all are flushed on
  shutdown; the rollup equals the raw table.
- **Isolation**: queries and the HTTP endpoint are tenant-scoped, and a cross-tenant request gets
  404.
- **Failure behaviour**: a full buffer drops events instead of blocking, and an outage never
  reaches callers.

## Decision
- **The `analytics_clickhouse` module**:
  - `AnalyticsSink::record` is a non-blocking `try_send`; batches go out by size or interval,
    retried, then dropped and counted;
  - `events` is a MergeTree with monthly partitions, ordered by `(organization_id, event, ts)`,
    with a 400-day TTL;
  - `events_daily` is a SummingMergeTree rollup kept for 1,100 days, fed by a materialized view;
  - reads require an `OrgAccess` proof.
- **Analytics is lossy by design.** Anything that must not be lost (runs, audit, billing records)
  stays in PostgreSQL, and analytics events are derived from those writes.
- The module is optional. The core profile keeps per-tenant dashboard counters in PostgreSQL
  (`runs` stats), which is fast enough at this scale (see the tenant row above).

## Alternatives considered
- **PostgreSQL only** (partitioned events table, BRIN indexes, or TimescaleDB):
  - competitive for single-tenant reads (5.5 vs 3.5 ms);
  - an order of magnitude slower for scans across tenants, 7.7× larger, and the analytics load
    competes with OLTP traffic on the primary.

  This is the right choice while events are few or reports are only per tenant. The reversal
  conditions below say when it stops being enough.
- **Writing analytics synchronously in the request**: rejected. It couples request latency and
  availability to the analytics store.
- **A hosted product-analytics SaaS**: out of scope for a self-hostable blueprint. The sink trait
  allows adding an exporter.

## Consequences
- A second datastore to operate when the module is on: backups, upgrades, sizing.
- Events can be lost when ClickHouse is down for longer than the retry window, or when the
  buffer overflows. Both cases are visible in `app_analytics_dropped_total{reason}`.

## Reversal conditions
- If event volume stays small (fewer than about 10M events) and reports are per tenant only,
  disable the module and use PostgreSQL.
- If at-least-once analytics becomes a requirement (for example, billing on events), put a
  durable queue in front (JetStream, ADR 0007), or write to PostgreSQL and replicate to ClickHouse.
