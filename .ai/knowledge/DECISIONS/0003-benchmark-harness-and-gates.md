# 0003: Own benchmark harness with invariant and same-machine regression gates

- Status: accepted
- Date: 2026-10-04

## Context
The blueprint promises that every performance technology is adopted because of evidence. That
covers caches, brokers, proxies, io_uring runtimes, allocators, LTO and PGO. Evidence needs four
things:
- a repeatable harness;
- results that record their environment;
- reports that cannot contain numbers nobody measured;
- gates that stop regressions.

Published benchmarks such as TechEmpower and vendor posts answer a different question: other
hardware, other code, tuned configurations. They are cited only as external evidence, and labelled
as such.

## Evidence
- Run-to-run noise on this machine, measured with repeated full runs: ≤7.6% throughput between
  full runs, up to 23% for single samples. Hence the median of 3 per point and regression
  tolerances wider than the noise (`benchmarks/README.md`).
- An earlier sequential allocator comparison looked 26–36% slower; interleaved A B A B runs showed
  it was environmental drift ([release-profile](../../../docs/benchmarks/release-profile.md)).
  Hence interleaving for any A/B decision whose expected effect is near the noise.
- The harness found defects that code review had not: seven outbound-controller bugs, breaker
  dilution, ops endpoints being rate-limited, rate limiting keyed by IP instead of principal
  ([summary](../../../docs/benchmarks/SUMMARY.md)).

## Decision
- `benchmarks/run.py` (Python stdlib) orchestrates the run:
  - builds release binaries;
  - starts an isolated stack on its own ports and its own `app_bench` database;
  - drives HTTP load with `oha` and samples server CPU and RSS;
  - runs `app-bench` (Rust) for outbound-engine and PostgreSQL workloads.

  Results are written to `benchmarks/results/<UTC>-<sha>.json` with the full environment and git
  dirty flag. A suite that cannot run is recorded as failed, with its reason.
- Gates (`benchmarks/gates.json`, `compare.py`) come in two kinds:
  - **Invariants**: correctness-under-load properties (zero errors, 401/404 on every rejected
    request, engine waste and amplification bounds). They are checked everywhere, including CI
    smoke runs.
  - **Regressions**: compared against `baseline.json` only when the run and the baseline share a
    machine fingerprint. Otherwise they are reported as "not comparable".
- `report.py` renders `docs/benchmarks/latest.md` from a result file. The docs are generated,
  never hand-written.
- Outbound benchmarks report *useful throughput* and *waste* against a naive client, not raw
  request rate.

## Alternatives considered
- **criterion micro-benchmarks only**: rejected as the primary tool. They cannot show
  system-level effects (middleware stack, pools, VM networking, controller dynamics), though they
  remain fine for hot functions.
- **k6 / wrk / Gatling**: rejected for now:
  - `oha` gives JSON output, percentiles and status distributions in a single binary;
  - k6 adds a JS runtime;
  - wrk needs Lua for JSON output.

  Any of them can replace `oha` behind `run.py`'s `oha()` function.
- **Throughput gates in shared CI**: rejected. Shared runners vary by more than the tolerances, so
  such gates would either flap or be set loose enough to be useless.

## Consequences
- Every "keep or reject" decision cites a result file, and the result file names the machine.
- Benchmarks immediately found and fixed seven controller bugs in the outbound engine. See commit
  "Engine: fix rate controller dynamics found by benchmark traces".
- The load generator shares the host with the server, so HTTP numbers are lower bounds. On macOS,
  services run in a VM, so database-bound numbers include VM networking.

## Reversal conditions
- If the team needs distributed load (multiple generator hosts) or long soak tests, replace the
  `oha` driver with k6 or Gatling. The result schema and gates stay the same.
- If a dedicated, quiet benchmark host becomes available, tighten the regression tolerances to its
  measured noise and gate throughput in CI on that host.
