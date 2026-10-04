# 0005: Outbound engine uses separate rate and concurrency controllers, judged by useful throughput

- Status: accepted
- Date: 2026-10-04

## Context
External APIs, such as LLM providers, payment APIs and commerce APIs, limit clients in two
independent ways:
- **request rate**: 429 responses, usually with `Retry-After`;
- **concurrency or capacity**: 503/504 responses, timeouts and rising latency.

The mission objective is maximum *successful useful throughput*, not maximum request rate.
Details of the design are in `docs/architecture/outbound-engine.md`.

## Decision
- **Concurrency**: an AIMD controller with a latency signal and drain-epoch decreases. It reacts
  only to 503/504, timeouts and sustained latency rise.
- **Rate**: a controller learned from 429s. It remembers a ceiling, holds at 95% of it for 30 s,
  then probes at +1%/s. It uses slow start until a reliable ceiling is known, and credits growth
  only for un-paused, utilised time.
- **Retry-After**: a 429 with `Retry-After` pauses the whole provider. Provider-directed waits do
  not spend the retry budget.
- **Breaker**: hard failures are judged over 10 s. Outages (≥95% failures of any kind) are judged
  over the last 1 s.
- **Metrics**: every benchmark reports useful req/s and waste (rejected ÷ sent), compared against
  a naive client.

## Evidence
`benchmarks/results/20261004T114357Z-23e824900844.json` and the trace runs described in commit
d06ac75 "Engine: fix rate controller dynamics found by benchmark traces".

- **Before** the separation and the dynamics fixes, against a 500 req/s provider, the engine
  completed all work at 163 useful req/s. Traces showed:
  - the concurrency limit collapsed to 1–2;
  - the cap jumped after pauses;
  - a cap of 1,000+ coexisted with 44 req/s actually sent.
- **After**: a steady 496–500 req/s (99–100% of the limit) and 1.8% waste over a 7,500-item run,
  including learning pauses. The naive client completes 9.9% of the work with 99.5% waste.
- **Overload** (capacity ≈1,600 req/s): the engine completes 100% of the work at 1,562 req/s with
  0.2% waste. The naive client completes 26.1% with 98.6% waste.
- **Outage**: the engine sends 0.48× the offered load to the dead provider and recovers 0.46 s
  after the provider does. The naive client sends 21×.
- Regression test: `learned_rate_converges_near_the_provider_limit` (≥85% of the limit at steady
  state; concurrency not collapsed).

## Alternatives considered
- **A single AIMD controller for every overload signal**: this was the previous design. Rejected:
  a rate limit is not a concurrency limit, and reacting to both compounded the backoff.
- **A fixed configured rate only**: kept as an upper bound (`requests_per_second`), but not
  sufficient alone. Provider limits differ per key and tier and change over time.
- **Gradient or Vegas-style concurrency control**: not needed so far. AIMD plus a latency floor
  converges to provider capacity in the benchmarks.

## Tradeoffs
- Learning costs a few `Retry-After` pauses at startup (3–4 at a 400–500 req/s limit).
- Controller state is per process. Instances that share one provider quota each learn
  separately: configure a per-instance `requests_per_second`, or use the shared Redis GCRA limiter.
- Fail-fast while the breaker is open loses requests that a brute-force client would complete
  at recovery (1,416 vs 1,600 in the outage scenario). Durable callers (the job queue) retry later.

## Reversal conditions
- If providers in use send rate-limit headers (`x-ratelimit-*`, IETF `RateLimit`), add header
  parsing first. It removes the learning pauses, and the learned controller becomes the fallback.
- If production traces show oscillation that the simulated scenarios do not, revisit the hold
  and probe constants using `app-bench trace-rate` against a recorded traffic shape.
