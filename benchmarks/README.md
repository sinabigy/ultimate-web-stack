# Benchmarks

The evidence base for every performance decision in this blueprint. A technology is added,
kept or rejected because of a number produced here (or a cited external source, labelled as
such in `docs/benchmarks/external-evidence.md`), never because it "sounds fast".

```
./dev benchmark                 # = python3 benchmarks/run.py, then compare + report
python3 benchmarks/run.py --smoke --suite http,outbound   # quick subset
python3 benchmarks/compare.py   # gates: invariants + regressions vs baseline.json
python3 benchmarks/report.py    # results/latest.json → docs/benchmarks/latest.md
python3 benchmarks/compare.py --update-baseline           # accept latest.json as baseline
```

Prerequisites: `./dev up --no-app` (PostgreSQL; Redis and Dragonfly for the cache suite) and
[`oha`](https://github.com/hatoo/oha) on `PATH`. `run.py` builds the release binaries itself.

## Layout

| path | what |
|---|---|
| `run.py` | orchestrator: builds, starts an isolated stack (ports 18090/59083, database `app_bench`), runs suites, samples server CPU/RSS, writes results |
| `../backend/apps/bench` | `app-bench`: outbound-engine scenarios, PostgreSQL workloads, HTTP fixture seeding, rate-controller trace |
| `gates.json` | invariants (always enforced) and regression tolerances (enforced on the same machine) |
| `compare.py` | applies the gates; exit 1 on failure |
| `report.py` | renders a result to Markdown |
| `results/*.json` | immutable result files, `<UTC time>-<git sha>[-smoke][-label].json`; `latest.json` / `latest-smoke.json` aliases |
| `baseline.json` | the accepted reference run for regression checks |

## Suites

| suite | measures | how |
|---|---|---|
| `http` | raw server overhead through the full middleware stack; PostgreSQL read/write per request; cached read | oha against `/bench/*` (enabled only with `APP__HTTP__BENCH_ENDPOINTS=true`), concurrency sweep 16/64/256 |
| `auth` | cost of authentication and authorisation; rejection paths | real BFF session cookie → session lookup → membership/permission check → tenant query; 401 and cross-tenant 404 |
| `cache` | memory vs Redis vs Dragonfly behind the same `CacheLayer` | `/bench/cached`, server restarted per backend |
| `outbound` | useful throughput and waste of the outbound engine vs a naive client | `fake-upstream` simulating keep-alive, rate limits (429 + Retry-After), overload (503) and outages |
| `db` | query cost, pool sizing, pool wait | `app-bench db`: real session lookup function, tenant list SQL, PK read, update |

## Methodology and honesty rules

- **Only measured numbers are recorded.** A suite that cannot run is stored as `failed`/`skipped`
  with the reason; reports print "Not measured".
- Every result carries its environment: CPU, cores, memory, OS, container VM, rustc, git SHA,
  dirty flag, load-generator version. Results from different machines are never compared by the
  gates.
- Release builds only (`lto = "thin"`, `codegen-units = 1`). A 2 s warm-up precedes each
  endpoint group.
- Requests still in flight when a timed run ends are cancelled by the load generator; they are
  excluded from success rates (they are an artefact of stopping, not failures).
- The load generator and the server share the host, so HTTP numbers are lower bounds of
  server capacity. PostgreSQL, Redis and Dragonfly run inside the container VM (macOS:
  colima), so database-bound numbers include VM networking and are bounded by the VM's CPUs.
- **Useful throughput** (completed work per second) is the outbound metric, not raw request
  rate: a client that sends 50,000 requests to get 700 successes is not fast.

## Regression gates

`gates.json` has two kinds of gate:

- **Invariants**: machine-independent properties. Examples: zero errors on healthy paths, every
  unauthenticated request gets a 401, every cross-tenant probe gets a 404, engine waste below
  bounds, no retry amplification during outages. They are enforced on every run, including CI
  smoke runs.
- **Regressions**: throughput, latency and memory relative to `baseline.json`, with per-metric
  tolerances. They are enforced only when the run and the baseline share a machine fingerprint.
  On other machines they are reported as not comparable rather than silently passing or failing.

CI runs the smoke suite with invariants only, because shared runners are too noisy for
throughput gates. Run full benchmarks on dedicated hardware, and record a baseline per machine.
