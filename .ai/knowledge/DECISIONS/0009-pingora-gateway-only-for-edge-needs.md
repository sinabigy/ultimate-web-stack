# 0009: Pingora gateway ships in the hyperscale profile, off by default; adopt it for edge capabilities, not speed

- Status: accepted
- Date: 2026-10-05

## Context
The brief lists Pingora for the hyperscale profile and requires direct vs proxied measurements
before adoption. A gateway can provide:
- a single entry point and health-checked balancing across instances;
- TLS termination;
- connection reuse to the app tier;
- programmable edge policies: routing, early rejection, tenant-aware limits.

## Evidence
See [docs/benchmarks/pingora.md](../../../docs/benchmarks/pingora.md): one host, median of 3, with
nginx at equal worker count as a baseline.

- **Trivial endpoints:** proxying through Pingora roughly halves throughput (143k → 66k req/s) and
  adds about 0.5 ms p50 at concurrency 64. The gateway uses about 45 µs CPU per request.
- **Database-backed endpoint:** 12.0k → 10.6k req/s, +0.75 ms p50.
- **nginx is within noise of Pingora** (65.5k vs 65.9k trivial; 11.1k vs 10.6k DB).

## Decision
- `backend/gateway` (Pingora 0.9) is a separate Cargo workspace, so the core build never compiles
  Pingora's dependency tree.
  - It is excluded from `backend/Cargo.toml` and is compile-checked by `gateway-check`.
  - It provides round-robin load balancing with TCP health checks, pooled upstream connections,
    timeouts and `X-Forwarded-For`.
- **Not in the default request path.** The core and performance profiles serve directly or behind
  whatever load balancer the platform provides (cloud LB, nginx, Caddy).
- **Adopt Pingora** when the edge needs custom Rust logic that off-the-shelf proxies make awkward,
  and when that logic runs on dedicated edge nodes. Examples:
  - tenant-aware routing or sharding;
  - early authentication or rejection before the app tier;
  - custom rate-limiting algorithms;
  - connection coalescing across many app instances.

## Alternatives considered
- **nginx / HAProxy / Envoy / cloud load balancers**: equal or better for standard proxying; nginx
  measured equivalent. They are the default recommendation (see deployment docs).
- **Doing nothing (direct exposure)**: fine for single-instance and VPS deployments behind a TLS
  terminator.

## Consequences
- Building the gateway needs `cmake` (via `libz-ng-sys`).
- Running it means operating one more tier, which needs to be sized at about 45 µs CPU per request
  on modern cores for trivial requests.

## Reversal conditions
- If a deployment's edge logic outgrows nginx or Envoy configuration (for example, per-tenant
  dynamic routing tables), move that logic into the Pingora gateway.
- If Pingora's measured per-request cost drops well below nginx's on the target hardware, revisit
  it as the default edge.
