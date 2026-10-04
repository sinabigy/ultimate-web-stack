# 0004: Sessions are looked up in PostgreSQL on every request; no Redis session cache

- Status: accepted
- Date: 2026-10-04

## Context
Every cookie-authenticated request hashes the session token and runs one indexed PostgreSQL
query that joins the session to its user (`app_db::sessions::find_active_with_user`). A
Redis or Dragonfly session cache is a common optimisation. The blueprint already ships a cache
layer (T-0008), and the mission asks for that layer to be justified by evidence.

Two invariants constrain this decision:
- invariant 3: the Rust backend is the final boundary;
- invariant 10: security operations leave evidence.

Together they require that revocation (logout, logout-all, suspension, admin revoke) takes
effect on the next request. Redis must never be the only source of session truth.

## Evidence
All numbers come from `benchmarks/results/20261004T114357Z-23e824900844.json`, recorded on an
Apple M5 host, with PostgreSQL 18 and Redis 8 in a 2-CPU VM and a release build.

| measurement | value |
|---|---|
| `session_auth_lookup` alone, pool 32, 64 workers | 7,038 ops/s; p50 8.5 ms, of which 7.0 ms is pool wait |
| `GET /api/v1/account/profile` (session + 1 query) | 4,252 req/s, p50 14.3 ms |
| `GET /api/v1/session` (session + 2 queries) | 2,828 req/s, p50 21.6 ms |
| `GET /api/v1/orgs/{slug}/runs` (session + membership + list) | 2,377 req/s, p50 25.8 ms |
| `/bench/db` (1 query, no auth) | 9,563 req/s |
| cached read via Redis vs in-process memory | 60,397 vs 129,336 req/s |

Throughput falls roughly in proportion to the number of database round trips per request:
4,252 req/s at 2 trips and 2,828 req/s at 3. The session lookup is one of those 2–3 trips, and is
neither the most expensive nor the dominant one. Removing it with a Redis cache would raise
these endpoints' ceiling by at most about one third. In exchange it would add a second
network hop per request, a second consistency domain, and a revocation-staleness window.

## Decision
- The session lookup stays in PostgreSQL. No Redis session cache is shipped or enabled.
- Redis and Dragonfly remain available through `CacheLayer` for application data, rate limiting
  and locks, where staleness is acceptable or explicitly bounded.
- The larger lever is fewer round trips per request. Examples: fold the membership-facts query
  into the organisation-scoped request; batch `unread_count` with the organisations list. That
  work is tracked as a performance follow-up and must be measured with `benchmarks/run.py --suite auth`.

## Alternatives considered
- **Redis session cache with PostgreSQL fallback**:
  - Revocation must then also invalidate Redis.
  - If the Redis delete fails or races a concurrent read, a revoked session stays usable until its
    TTL expires. That means failing open, which the T-0008 criterion explicitly forbids.
  - Rejected while the measured gain is at most ~33%.
- **In-process session cache with a short TTL (≤5 s) plus revocation fan-out over the event bus**:
  - Cheaper than Redis (no network hop).
  - It still leaves a bounded staleness window, and with several instances it depends on every
    instance receiving the revocation event.
  - This is the preferred option if the reversal condition below is met.
- **Stateless JWT sessions**: rejected. They make revocation impossible without a denylist,
  which brings back the same lookup, and they put tokens in the browser, against the BFF
  model (invariant 4).

## Consequences
- Revocation is immediate, with no cache invalidation protocol.
- Every authenticated request costs one indexed PostgreSQL query, which scales with read
  replicas and connection pooling.
- The T-0008 criterion "Redis session cache (if enabled) falls back to PostgreSQL without failing
  open" is satisfied by construction: no session cache is enabled.

## Reversal conditions
Revisit this decision if, at production scale, either of these holds:
- `pg_stat_statements` shows the session lookup above ~30% of total query time;
- the authenticated p99 budget is violated while PostgreSQL CPU is saturated and read replicas
  are not an option.

Then implement the in-process option. It must have a TTL ≤5 s, revocation events on the
`EventBus`, and a benchmark plus a revocation-latency test showing a revoked session is rejected
within the TTL on every instance.
