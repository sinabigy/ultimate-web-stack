# Failure modes: expected degradation and evidence

Design rule: **PostgreSQL is the only critical dependency.** If it is down, the API answers 503
(retryable) and recovers by itself. Every optional module degrades without taking the core down;
readiness reports it as `degraded` (HTTP 200) so operators see the problem but traffic keeps
flowing.

Two kinds of evidence:
- **Test**: a test name, in `backend/crates/*/tests/`, which runs in `./dev check`.
- **Drill**: `./dev test --drill` (`scripts/failure_drill.py`) against a running full stack. It
  stops real containers and injects provider faults. Last run: 2026-10-05, 14/14 pass.

| fault | expected behaviour | evidence |
|---|---|---|
| PostgreSQL down | `/readyz` and DB-backed endpoints answer **503** `unavailable` immediately: no hang, no crash. Sessions survive, and the app recovers when the database returns. SQLSTATE `57P01`/`57P02`/class `08` and protocol errors also count as unavailable. | test `database_outage_yields_503_not_crash`, `app-db` `outages_are_unavailable_not_internal`; drill "PostgreSQL down/back" |
| NATS down (messaging module) | readiness `degraded` (`messaging`); runs and the API keep working; the client reconnects. NATS unreachable at startup: the server starts on the PostgreSQL event bus and reports degraded. | test `nats_unavailable_at_startup_falls_back_to_postgres_events`; drill "NATS down/back" |
| ClickHouse down (analytics module) | readiness `degraded` (`analytics`). Ingestion never blocks callers: the bounded buffer drops and counts. Analytics endpoints fail alone. | test `a_full_buffer_drops_instead_of_blocking_and_an_outage_never_reaches_callers`; drill "ClickHouse down/back" |
| Redis down (cache module) | cache reads fall through to the source; requests do not fail | test `cache_outage_degrades_to_source_without_failing_requests` |
| Identity provider down | readiness `degraded` (`identity_provider`); existing sessions keep working; new logins go back to `/login` with a reason | test `identity_provider_outage_sends_browser_back_to_login_with_reason` |
| Provider 429 | the engine learns the limit, keeps useful throughput near it, and completes the work | test `rate_limited_provider_is_respected_and_work_completes`, `learned_rate_converges_near_the_provider_limit`; drill: 60/60 calls at a 20 req/s limit |
| Provider 5xx | idempotent calls retried with backoff; persistent failures counted; the run finishes | drill: 30% errors → run completed, 38–39/40 succeeded |
| Provider slow / timeout | per-request deadline and total deadline bound the run; it fails instead of hanging | test `deadlines_bound_total_time`; drill: 3 calls at 30 s latency → failed in 41 s |
| Provider outage | breaker opens; amplification bounded (≤ 1.5 requests per call measured, versus 4 for a naive client with 3 retries) | test `outage_opens_circuit_fails_fast_then_recovers`; drill: 300 requests for 200 calls |
| Non-idempotent call fails | never retried (no duplicate side effects) | test `non_idempotent_posts_are_not_retried` |
| Worker crash or restart mid-run | lease expires, another worker resumes **without redoing finished work**; expired leases reaped | tests `interrupted_run_resumes_on_another_worker_without_redoing_work`, `expired_leases_are_reaped`; systemd live test (worker SIGKILL → restarted) |
| Duplicate job / message | enqueue idempotent per key; JetStream dedup by `Nats-Msg-Id`; every message processed once | tests `job_enqueue_is_idempotent`, `idempotency_key_deduplicates_publishes`, `every_message_is_processed_and_acknowledged_once`, `concurrent_claims_never_share_a_job` |
| Poison or unknown job | backoff, then dead letter; replayable (JetStream) | tests `poison_and_unknown_jobs_are_dead_lettered`, `failures_back_off_then_dead_letter`, `exhausted_and_permanent_failures_are_dead_lettered_and_replayable`, `a_crash_on_the_last_attempt_is_swept_to_the_dead_letter_stream` |
| Malformed event | ignored and counted; the bus keeps working | test `malformed_events_are_ignored_and_the_bus_keeps_working` |
| Queue backlog | drains in order under bounded concurrency | drill: 25 runs × 20 calls drained in 58 s |
| Invalid or forged session | 401; no information about why | drill "tampered session cookie → 401"; tests in `auth_flow.rs` |
| Missing CSRF token | 403 `csrf_failed` on any state-changing request | drill; browser acceptance "session" test |
| Shutdown (SIGTERM) | not-ready, drain in-flight requests and jobs, exit 0 | test `graceful_shutdown_drains_then_stops`, `graceful_shutdown_finishes_in_flight_work`; release smoke and systemd live test |

What is **not** covered: network partitions between API instances, disk-full on the database
host, and a slow PostgreSQL (rather than a stopped one) under production load. The statement
timeout (`statement_timeout_applied_to_pool_connections`) bounds the last one.
