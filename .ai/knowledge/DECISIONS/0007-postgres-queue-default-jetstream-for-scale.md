# 0007: PostgreSQL job queue by default; NATS JetStream for scale-out and cross-service work

- Status: accepted
- Date: 2026-10-04

## Context
The blueprint needs durable background work: retries, dead letters, idempotency and graceful
shutdown. It ships two implementations:
- **PostgreSQL** (`backend/crates/workers`): `SKIP LOCKED` claims, leases with heartbeat,
  LISTEN/NOTIFY wake-ups, a reaper and a dead state;
- **NATS JetStream** (`backend/crates/messaging/src/nats.rs`, feature `nats`):
  - work-queue stream, durable pull consumer, explicit acks;
  - `Nats-Msg-Id` dedup and backoff naks;
  - dead-letter stream with replay;
  - a sweeper for messages whose last delivery was never acked.

## Evidence
Measured with `app-bench queue` (`benchmarks/run.py --suite messaging`): 20,000 no-op jobs,
16 concurrent producers with idempotency keys, 32 concurrent handlers. Both brokers ran in the
same 2-CPU container VM, with file/WAL durability. Three consecutive ad-hoc runs, plus the recorded
harness run `benchmarks/results/20261004T133540Z-16e774555144-queue.json` (PostgreSQL
3,180 / 4,654; JetStream 43,899 / 46,647; all 20,000 processed):

| | publish/s | drain/s |
|---|---|---|
| PostgreSQL queue | 2,828 – 3,296 | 4,418 – 5,148 |
| JetStream | 35,065 – 47,020 | 37,933 – 49,138 |

Every run processed all 20,000 jobs on both backends.

Correctness of the JetStream path, `backend/crates/messaging/tests/nats.rs` (8 tests, stable over 5
repetitions):
- dedup and exactly-once processing on the happy path;
- retry with backoff, dead-lettering after `max_deliver`, and immediate dead-lettering for
  permanent failures;
- replay from the dead-letter stream;
- crash on the last attempt swept to the dead-letter stream;
- graceful drain;
- trace propagation through headers.

The crash test also fails when the progress-heartbeat guard is removed (mutation-checked).

## Decision
- **The PostgreSQL queue is the default.** It needs no extra infrastructure. Jobs are enqueued
  *in the same transaction* as the business row (for example, a run and its `execute_run` job),
  so no job is lost or orphaned. At about 4–5k jobs/s per database it covers the core profile by a
  wide margin.
- **JetStream is the scale-out module** (`messaging_nats`). Use it when one of these holds:
  1. sustained job volume is a meaningful fraction of the PostgreSQL rate on your hardware (it
     competes with OLTP traffic for the same database);
  2. consumers live in other services or languages;
  3. you need fan-out, replay or stream retention.
- **With `messaging.enabled`, realtime events go over NATS** (`NatsEventBus`) instead of
  `pg_notify`. That removes database load and the 8 KB NOTIFY payload limit.
- **Example-app runs stay on PostgreSQL even when NATS is enabled**, because the transactional
  enqueue matters more than throughput there. Publishing to JetStream after commit would need an
  outbox relay to stay correct.

## Alternatives considered
- **JetStream for everything**: rejected as the default. It adds infrastructure for small
  deployments, and loses transactional enqueue unless an outbox is added.
- **Redis Streams**: rejected. Weaker durability defaults, no built-in dedup window, and Redis is
  already optional as a cache only.
- **Kafka or Redpanda**: rejected for this blueprint. Heavier to operate; the throughput needs
  measured here are met by JetStream.

## Consequences
- Two queue runtimes with the same semantics (retryable vs permanent errors, at-least-once,
  idempotent handlers).
- Producers that need transactional guarantees *and* JetStream must use an outbox (insert in
  PostgreSQL, then relay). This is not implemented yet; it is listed as a known limitation.

## Reversal conditions
- If production job volume makes the PostgreSQL queue a measurable share of database load, move
  those job kinds to JetStream with an outbox relay.
- If NATS operations cost more than the scale benefit, for example on a single small instance,
  keep `messaging.enabled = false`.
