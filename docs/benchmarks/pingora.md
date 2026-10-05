# Pingora gateway: direct vs proxied

Source: `benchmarks/results/20261004T140349Z-0cd96a42816b-pingora-vs-nginx.json`. Reproduce with
`python3 benchmarks/run.py --suite gateway`.

Setup:
- `backend/gateway` (Pingora 0.9, round-robin load balancing plus TCP health checks, 4 threads)
  in front of one `app-server`;
- nginx 1.31.6 as a baseline (4 workers, upstream keep-alive 256);
- oha with keep-alive, median of 3 runs per point, release builds;
- everything on one Apple M5 (10 cores), so the load generator, proxy and app compete for CPU.

| endpoint | conc. | direct req/s | Pingora req/s | nginx req/s | direct p50 / p99 ms | Pingora p50 / p99 ms | nginx p50 / p99 ms |
|---|---:|---:|---:|---:|---|---|---|
| `/bench/plaintext` | 64 | 143,115 | 65,864 | 65,531 | 0.42 / 1.11 | 0.92 / 1.78 | 0.84 / 2.09 |
| `/bench/plaintext` | 256 | 152,389 | 68,616 | 64,073 | 1.53 / 4.59 | 3.46 / 7.69 | 3.50 / 7.98 |
| `/bench/json` | 64 | 141,359 | 65,412 | 65,774 | 0.42 / 1.03 | 0.94 / 1.79 | 0.85 / 2.05 |
| `/bench/json` | 256 | 151,735 | 67,465 | 62,556 | 1.53 / 4.70 | 3.56 / 7.89 | 3.51 / 8.14 |
| `/bench/db` | 64 | 12,015 | 10,570 | 11,106 | 4.98 / 10.30 | 5.73 / 11.42 | 5.38 / 10.85 |
| `/bench/db` | 256 | 12,267 | 10,430 | 11,400 | 19.96 / 32.17 | 22.84 / 38.55 | 21.19 / 34.76 |

Gateway resources while proxying (`results/20261004T135911Z-0cd96a42816b-pingora.json`):
- **Trivial endpoints:** about 3 cores (290–306%) at around 66k req/s, which is about 45 µs of
  CPU per proxied request, and 15–28 MB RSS.
- **DB endpoint:** 0.65 core at around 10.5k req/s.

## Reading

- **An extra hop is not free.**
  - On trivial endpoints, a proxy on the same host costs about as much CPU per request as the app
    itself, so end-to-end throughput roughly halves.
  - It adds about 0.5 ms p50 at concurrency 64.
  - On database-backed endpoints the cost is 7–15% throughput (nginx 7–8%, Pingora 12–15%) and about 1–3 ms.
- **Pingora and nginx are equivalent here** at equal worker counts. Pingora is slightly ahead at
  high concurrency on trivial routes; nginx is slightly ahead on the database route. Neither is a
  reason to choose one over the other.
- In production, the edge runs on its own nodes, so it does not steal CPU from the app. The
  per-request CPU cost then becomes a sizing input rather than a throughput loss.

Decision and when to adopt:
[ADR 0009](../../.ai/knowledge/DECISIONS/0009-pingora-gateway-only-for-edge-needs.md).
