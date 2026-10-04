# Benchmark results

Generated from `benchmarks/results/20261004T121126Z-4ec088c960a7.json` by `benchmarks/report.py`. Do not edit by hand.

## Environment

|  |  |
|---|---:|
| when | 2026-10-04T12:11:26+00:00 → 2026-10-04T12:20:07+00:00 |
| machine | Apple M5 · 10 logical cores · 16.0 GB · macOS 26.5.1 · arm64 |
| services | container VM: 2 cpus, 4094447616 bytes, Ubuntu 24.04.4 LTS, aarch64 |
| build | release (lto=thin, codegen-units=1) · rustc 1.99.0 (b940084d7 2026-09-28) |
| code | `4ec088c960a7` |
| load generator | oha 1.16.0 |

> Load generator and server share the host; services (PostgreSQL/Redis/Dragonfly) run in the container VM.

## HTTP server

oha, 6s per point, keep-alive, full middleware stack (request id, tracing, security headers, CSRF, metrics, timeouts, load shedding).

| endpoint | conc. | success req/s | success | p50 ms | p99 ms | p99.9 ms | server CPU | RSS MB | req/s per core |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `/bench/plaintext` | 16 | 111,239 | 100.00% | 0.13 | 0.44 | 1.29 | 334% | 38.6 | 33,335 |
| `/bench/plaintext` | 64 | 136,321 | 100.00% | 0.43 | 1.20 | 4.29 | 367% | 58.5 | 37,135 |
| `/bench/plaintext` | 256 | 130,593 | 100.00% | 1.72 | 6.21 | 16.36 | 336% | 40.8 | 38,844 |
| `/bench/json` | 16 | 113,371 | 100.00% | 0.12 | 0.41 | 1.10 | 340% | 77.0 | 33,305 |
| `/bench/json` | 64 | 132,478 | 100.00% | 0.43 | 1.33 | 5.15 | 351% | 63.0 | 37,722 |
| `/bench/json` | 256 | 131,135 | 100.00% | 1.68 | 6.38 | 21.98 | 340% | 62.1 | 38,592 |
| `/bench/db` (1 indexed read) | 64 | 9,496 | 100.00% | 6.42 | 12.36 | 16.39 | 71% | 42.3 | 13,375 |
| `/bench/updates` (1 read-modify-write) | 64 | 9,146 | 100.00% | 6.73 | 12.48 | 15.03 | 71% | 36.2 | 12,828 |
| `/bench/cached` (memory cache) | 64 | 131,803 | 100.00% | 0.45 | 1.25 | 4.19 | 386% | 65.2 | 34,111 |
| `/healthz` | 64 | 139,257 | 100.00% | 0.42 | 1.13 | 3.59 | 366% | 78.5 | 38,090 |

`req/s per core` = success req/s ÷ (server CPU% / 100): a derived ratio, sensitive to `ps` sampling.

## Authenticated requests

Real cookie → session → authorization path (BFF session cookie, no mocks).

| endpoint | DB round trips | conc. | success req/s | status codes | p50 ms | p99 ms | p99.9 ms | server CPU | RSS MB | req/s per core |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `GET /api/v1/account/profile` | 2 | 64 | 5,682 | 200: 34,108 | 10.88 | 18.91 | 25.08 | 74% | 24.4 | 7,678 |
| `GET /api/v1/session` | 3 | 64 | 3,636 | 200: 21,828 | 17.00 | 28.39 | 34.67 | 68% | 26.4 | 5,339 |
| `GET /api/v1/orgs/{slug}/runs` (tenant page of 20) | 3 | 64 | 2,903 | 200: 17,427 | 21.25 | 36.37 | 42.41 | 68% | 24.6 | 4,295 |
| no credentials → 401 | 0 | 64 | 0 | 401: 764,297 | 0.46 | 1.41 | 4.88 | 355% | 56.6 | 0 |
| non-member org → 404 (audited) | – | 16 | 0 | 404: 19,494 | 3.05 | 7.37 | 11.14 | 65% | 47.6 | 0 |

## Cache backends

/bench/cached (hot set of 100 keys, read-through to PostgreSQL), 6s per backend.

| backend | conc. | success req/s | success | p50 ms | p99 ms | p99.9 ms | server CPU | RSS MB | req/s per core |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| memory | 64 | 125,457 | 100.00% | 0.47 | 1.32 | 4.27 | 372% | 43.5 | 33,761 |
| redis | 64 | 57,131 | 100.00% | 1.08 | 1.97 | 3.97 | 230% | 46.7 | 24,872 |
| dragonfly | 64 | 53,727 | 100.00% | 1.14 | 2.25 | 5.14 | 215% | 32.2 | 24,943 |

## Outbound engine

Simulated provider on loopback (`fake-upstream`). *Naive* = fixed concurrency, immediate retry on any failure (up to 20), no rate/concurrency control, no breaker.

### Connection reuse

| client | requests | TCP connections | reuse | useful req/s | p50 ms | p99 ms |
|---|---:|---:|---:|---:|---:|---:|
| fresh_connections | 10,000 | 10,000 | 0.0% | 26,004 | 1.86 | 2.81 |
| pooled_keepalive | 10,000 | 64 | 99.4% | 40,890 | 0.81 | 1.87 |

### Concurrency sweep (provider latency 5 ms)

| concurrency | requests | useful req/s | p50 ms | p99 ms | p99.9 ms |
|---|---:|---:|---:|---:|---:|
| 1 | 800 | 137 | 7.63 | 7.75 | 9.17 |
| 8 | 800 | 1,089 | 7.68 | 7.77 | 7.81 |
| 32 | 2,560 | 4,156 | 7.82 | 8.14 | 8.20 |
| 128 | 10,240 | 15,462 | 7.32 | 10.11 | 11.11 |
| 512 | 40,000 | 21,967 | 15.70 | 25.59 | 32.03 |

### Rate-limited provider (500 req/s, burst 50, 429 + Retry-After: 1)

| client | work items | completed | wall s | useful req/s | requests sent | rejected | waste | p99 ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| engine | 7,500 | 100.0% | 19.85 | 378 | 7,647 | 147 | 1.9% | 1,840 |
| naive | 7,500 | 9.3% | 1.30 | 537 | 150,038 | 149,343 | 99.5% | 32.7 |

### Overloaded provider (16 concurrent at 10 ms ≈ 1,600 req/s capacity; 503 above 48 in flight)

| client | work items | completed | wall s | useful req/s | requests sent | rejected | waste | p99 ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| engine | 15,000 | 100.0% | 9.61 | 1,561 | 15,028 | 28 | 0.2% | 167.1 |
| naive | 15,000 | 22.0% | 2.14 | 1,545 | 279,685 | 276,380 | 98.8% | 63.8 |

### Provider outage (open loop 400 req/s for 6 s; 503 from 1–3 s)

| client | offered | succeeded | requests to dead provider | amplification | recovery |
|---|---:|---:|---:|---:|---:|
| engine | 2,400 | 1,415 | 383 | 0.48× | 0.462 s |
| naive | 2,400 | 1,600 | 16,800 | 21.00× | 0.006 s |

Amplification = requests sent to the provider during the outage ÷ requests offered in that window.

## PostgreSQL

`app-bench db`, 8 s per point. Latency includes pool acquire.

| workload | pool | workers | ops/s | p50 ms | p99 ms | pool wait p50 | pool wait p99 |
|---|---:|---:|---:|---:|---:|---:|---:|
| `pk_read` | 8 | 64 | 5,687 | 11.10 | 15.01 | 10.62 | 14.44 |
| `pk_read` | 32 | 64 | 10,007 | 6.13 | 11.64 | 5.10 | 9.74 |
| `pk_read` | 32 | 256 | 10,321 | 24.33 | 34.37 | 23.32 | 33.01 |
| `pk_read` | 64 | 256 | 11,366 | 22.02 | 36.42 | 20.30 | 34.01 |
| `tenant_list_20` | 8 | 64 | 5,244 | 11.99 | 17.31 | 11.45 | 16.54 |
| `tenant_list_20` | 32 | 64 | 8,812 | 7.04 | 12.48 | 5.85 | 10.40 |
| `tenant_list_20` | 32 | 256 | 8,802 | 28.41 | 47.79 | 27.20 | 45.78 |
| `tenant_list_20` | 64 | 256 | 9,126 | 27.32 | 45.33 | 25.27 | 41.00 |
| `session_auth_lookup` | 8 | 64 | 5,205 | 12.03 | 17.56 | 11.48 | 16.82 |
| `session_auth_lookup` | 32 | 64 | 8,111 | 7.62 | 13.50 | 6.33 | 11.08 |
| `session_auth_lookup` | 32 | 256 | 8,150 | 30.68 | 47.10 | 29.37 | 45.30 |
| `session_auth_lookup` | 64 | 256 | 7,979 | 31.44 | 46.35 | 29.14 | 41.53 |
| `update_one` | 8 | 64 | 4,684 | 13.40 | 19.12 | 12.57 | 17.95 |
| `update_one` | 32 | 64 | 8,258 | 7.58 | 12.76 | 5.51 | 9.04 |
| `update_one` | 32 | 256 | 7,871 | 31.23 | 53.49 | 29.08 | 49.63 |
| `update_one` | 64 | 256 | 8,634 | 28.86 | 50.50 | 24.54 | 43.44 |

## Gates

```
Invariants
  PASS  plaintext: no errors                                       1.0
  PASS  db read: no errors                                         1.0
  PASS  cached read: no errors                                     1.0
  PASS  authenticated request: no errors                           1.0
  PASS  missing credentials are always 401                         764297
  PASS  cross-tenant probes are always 404                         19494
  PASS  engine: keep-alive reuse                                   0.9936
  PASS  engine: rate-limited work completes                        1.0
  PASS  engine: 429 waste bounded                                  0.01922322479403688
  PASS  engine: overload work completes                            1.0
  PASS  engine: overload waste bounded                             0.0018631887143997872
  PASS  engine: overload goodput ≥ 85% of provider capacity (1600/s) 1561.1561792025625
  PASS  engine: no retry amplification during outage               0.47875
  PASS  engine: recovers within 2s of provider recovery            0.462129375

Regressions
  PASS  plaintext throughput                                       136321 → 136321 (+0.0%, tolerance 15%)
  PASS  plaintext p99                                              1.197 → 1.197 (+0.0%, tolerance 35%)
  PASS  json throughput                                            132478 → 132478 (+0.0%, tolerance 15%)
  PASS  db read throughput                                         9496.4 → 9496.4 (+0.0%, tolerance 20%)
  PASS  db read p99                                                12.364 → 12.364 (+0.0%, tolerance 40%)
  PASS  cached read throughput                                     131803 → 131803 (+0.0%, tolerance 15%)
  PASS  authenticated tenant page throughput                       2903.4 → 2903.4 (+0.0%, tolerance 20%)
  PASS  authenticated tenant page p99                              36.373 → 36.373 (+0.0%, tolerance 40%)
  PASS  server memory under load                                   58.5 → 58.5 (+0.0%, tolerance 60%)
  PASS  engine rate-limited completion time                        19.8545 → 19.8545 (+0.0%, tolerance 25%)
  PASS  session lookup ops/s                                       8110.62 → 8110.62 (+0.0%, tolerance 20%)

PASSED: 0 gate(s) failed
```

