# Alerting

`infra/docker/observability/alerts.yml` holds Prometheus rules for the application's own metrics.
They are symptom-based: each one names what users or operators lose. `alerts.test.yml` proves
that every rule fires on its symptom and stays quiet on healthy or low traffic, with
`promtool test rules`. `infra/verify.sh` runs it, and so does `./dev check` through `infra-verify`.

- **Development:** `./dev up --with observability` loads the rules into Prometheus. Firing alerts
  appear under *Alerts* in Prometheus and in Grafana's alert list.
- **Production:** load the same file into your Prometheus (or Grafana Mimir, or any compatible
  ruler). Then route the alerts to people with Alertmanager, Grafana alerting or a hosted service.
  Routing depends on who is on call and where, so the blueprint does not ship an Alertmanager
  ([decision](#what-is-not-included)).

Thresholds are starting points for a small deployment. Rate-based rules carry a traffic floor so a
single error at night does not page. Tune the numbers to your traffic, and keep the unit tests in
step.

| alert | severity | fires when |
|---|---|---|
| [AppDown](#appdown) | critical | an API instance misses scrapes for 2 min |
| [WorkerDown](#workerdown) | warning | a worker misses scrapes for 5 min |
| [HighServerErrorRate](#highservererrorrate) | critical | more than 5% 5xx for 5 min, at ≥ 0.5 req/s |
| [HighLatency](#highlatency) | warning | p99 above 1 s for 10 min, at ≥ 0.5 req/s |
| [LoadShedding](#loadshedding) | warning | requests shed at `http.max_inflight` for 5 min |
| [AuditWriteFailures](#auditwritefailures) | critical | any audit record could not be written |
| [JobsDeadLettered](#jobsdeadlettered) | warning | a job was dead-lettered (per kind) |
| [ProviderCircuitOpen](#providercircuitopen) | warning | an outbound provider's breaker opened |
| [EventBusFallback](#eventbusfallback) | warning | NATS unreachable; PostgreSQL fallback active |
| [CacheErrors](#cacheerrors) | warning | the cache backend keeps failing |
| [RateLimiterErrors](#ratelimitererrors) | warning | the rate limiter's store fails (requests are allowed) |
| [AnalyticsDropping](#analyticsdropping) | info | analytics events dropped |

## Runbooks

### AppDown
Prometheus cannot scrape the instance. Check the process or pod (`systemctl status app-server`,
`kubectl get pods`), then its logs. If it restarts in a loop, it usually logs `invalid
configuration` (run `app-server check-config`) or cannot reach PostgreSQL. A running process that
is not scraped points at the network or at `http.ops_port`: with an ops port, `/metrics` is served
only there.

### WorkerDown
Jobs and runs stop progressing; the API keeps answering. Leases from the dead worker expire and
another worker resumes the work without redoing finished steps
([failure modes](failure-modes.md)). Restart the worker and check its logs.

### HighServerErrorRate
Group `app_http_requests_total{status=~"5.."}` by `route`. A database outage shows as 503 on every
DB-backed route together with `/readyz` = 503 (the app recovers by itself when PostgreSQL returns).
One route failing points at that handler: find the cause through the `request_id` in the logs and
the trace linked from Grafana.

### HighLatency
Compare `app_http_request_duration_seconds` by route. The usual causes are slow queries (see
PostgreSQL's `pg_stat_statements`), pool saturation, and a slow provider called inline. A trace of
a slow request (Tempo) shows which span takes the time.

### LoadShedding
The instance is at `http.max_inflight` and answers 503 instead of queueing (that is the design).
Add instances, or find what holds requests open (latency above). Raise the limit only with a
measurement.

### AuditWriteFailures
Security operations must leave audit evidence (invariant 10). Changes are audited in the same
transaction, so they roll back rather than go unaudited. This metric counts the best-effort
records written outside a transaction: denials and observations. Check PostgreSQL health and disk
space first, then the logs (`failed to write audit event`, with the action). Reconstruct the missing
denials from those log lines.

### JobsDeadLettered
A job exhausted its retries or failed permanently. `GET /api/v1/admin/jobs?status=dead` (or the
admin console's *Jobs* page) shows the error. Fix the cause, then retry from the console. An
unknown job kind means a worker runs older code than the producer.

### ProviderCircuitOpen
An outbound provider failed repeatedly. The breaker fails calls fast, which bounds the extra load
on the provider, and closes again on its own. Check the provider's status page and
`app_provider_requests_total{outcome}`. Runs that depend on it finish as failed or partial; they do
not hang.

### EventBusFallback
The messaging module cannot reach NATS. Events flow through PostgreSQL, readiness reports
`degraded`, and the client reconnects by itself. Restore NATS, then confirm the alert clears.

### CacheErrors
The cache backend (Redis or Dragonfly) is failing. Reads fall through to the source, so requests
still succeed, but the database takes the load. Restore the cache before load rises.

### RateLimiterErrors
The rate limiter's store is failing. The limiter fails open, so requests are allowed without
limits; that keeps the service available but removes the protection. Restore the store promptly.

### AnalyticsDropping
ClickHouse is unreachable or the bounded buffer is full. Events are dropped by design, so callers
never block. Restore ClickHouse; dropped events are not replayed.

## What is not included

- **Alertmanager** (or any other router): who gets paged, through which channel and with which
  silences, depends on the deployment, and a development stack has nobody to page. The rules are
  the portable part.
- **Tail sampling.** Traces are exported with head sampling (`telemetry.trace_sample_ratio`, all of them
  in development). Tail sampling keeps all slow or failed traces and samples the rest. It needs a
  collector that buffers whole traces (Alloy's or the OpenTelemetry Collector's `tail_sampling`
  processor), sized to traffic, so it belongs in production collector configuration and not in the
  blueprint. Its value depends on trace volume we have not measured in production. To adopt it,
  point `telemetry.otlp_endpoint` at a collector running `tail_sampling`, with policies for
  `status_code: ERROR` and `latency > 1s` plus a probabilistic share of the rest.
- **Infrastructure alerts** (disk, memory, PostgreSQL replication): use the exporters of your
  platform; they are not specific to this application.
