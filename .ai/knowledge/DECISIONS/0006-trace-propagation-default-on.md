# 0006: W3C trace propagation on by default; OTLP export opt-in

- Status: accepted
- Date: 2026-10-04

## Context
The brief requires distributed tracing, with context propagated from request → queue → worker →
provider, and an OTel/Tempo stack. The tracing-opentelemetry layer costs CPU on every span,
including when nothing is exported. That cost has to be measured, not assumed.

## Decision
- `telemetry.trace_propagation = true` by default. Every request gets a W3C trace:
  - inbound `traceparent` is honoured;
  - the trace id is recorded on the `http` and `job` spans, so JSON logs carry `trace_id`;
  - jobs store the context in `jobs.trace_context`;
  - workers resume it;
  - the outbound engine injects `traceparent` per provider attempt, under a `provider.call`
    client span.
- Export is opt-in. `telemetry.otlp_endpoint` (OTLP/HTTP protobuf, batch processor) sends spans
  to an OpenTelemetry Collector or to Tempo. `trace_sample_ratio` samples new root traces;
  incoming sampling decisions are respected (parent-based sampling).

## Evidence
- **Micro-benchmark** (`app-bench span-cost`, release, 300k iterations, three runs): a request
  shaped like an `http` span plus one child span, with parent extraction, costs:
  - 255 ns with the plain registry;
  - 1,716 ns with the OpenTelemetry layer and no exporter;

  So the OpenTelemetry layer adds **≈1.46 µs of CPU per request**.
- **End-to-end A/B** (`results/*-otel-on.json` vs `*-otel-off.json`, median of 3):
  - trivial endpoints at concurrency 256 were 7–10% slower with tracing on (plaintext 146k → 131k
    req/s; json 136k → 125k);
  - DB-backed and authenticated endpoints differed by less than run-to-run noise.

  This matches the micro-benchmark: 1.46 µs is about 5% of a ~27 µs no-op request, but under 1%
  of a database-backed request.
- **Correctness**: `backend/crates/api/tests/tracing.rs` proves that one trace id goes from the
  inbound request through the job row to every provider call. It also proves that requests
  without a context start a new trace that still propagates. A mutation check (injection
  disabled) makes both tests fail.

## Alternatives considered
- **Off by default, on only when exporting**: this saves about 1.5 µs per request. But logs lose
  `trace_id`, and downstream services cannot join traces unless every deployment remembers to turn
  it on. Rejected as the default; it stays available as `trace_propagation = false`.
- **Request-id-only correlation**: kept as well, since jobs also store `request_id`. It does not
  give cross-service traces.

## Consequences
- About 1.5 µs CPU per request. That is negligible for real endpoints, and visible only on no-op
  endpoints at more than 100k req/s per instance.
- An external caller can force sampling with a `traceparent` flag (threat model residual risk).
  Proxies at a public edge may strip or re-root it.

## Reversal conditions
- If profiling a production workload shows tracing above 3% of CPU, either set
  `trace_propagation = false` for that service (and lose log correlation), or move to sampled span
  creation once tracing-opentelemetry supports skipping unsampled spans cheaply.
