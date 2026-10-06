# 0012: Compression at build time for static files, on the fly for API responses; private API caching; no API ETags

- Status: accepted
- Date: 2026-10-06

## Context
The server already negotiated precompressed static files (`ServeDir::precompressed_br/gzip`), but
the build never produced them. A deployment without a compressing proxy therefore sent the SPA
uncompressed. That covers the container image behind a load balancer and nginx with its default
`gzip_proxied off`; only the systemd profile's Caddy compresses. API JSON was never compressed.
API responses carried no `Cache-Control`, and the built `*.map` files were served publicly.

## Evidence
Payloads are real responses from a generated project (30 runs, audit trail): session 745 B, members
295 B, permissions 2.5 KB, dashboard 6.2 KB, runs 8.9–10.5 KB, audit page 13 KB.

- **Level, per 13 KB audit page (Apple M5, single core, node:zlib):** gzip-6 2,189 B in 41 µs;
  brotli-4 1,937 B in 50 µs; brotli-11 1,789 B in **10.9 ms**. Brotli-11 is right at build time
  and ruinous per request. tower-http's defaults (brotli 4, gzip 6) are the right runtime levels.
- **Static build** (9 JS/CSS files): 186 KB raw, 60 KB gzip-9, 53 KB brotli-11 (−71%), for 0.8 s
  of build time and no runtime CPU. The source maps were 609 KB.
- **Throughput** (`benchmarks/run.py --suite compression --repeat 5`, layer off / on / off, 64
  connections, loopback; `benchmarks/results/20261006T062201Z-*-compression.json`):
  - small JSON (below the 1 KiB threshold): 113k → 92k → 92k req/s. The drop is host drift; the
    second "off" run matches "on".
  - 13 KB list: off 12.48k / 12.45k req/s. On: identity 11.42k, brotli 11.76k, gzip 12.45k.
    These differences sit inside the run-to-run spread of about 10%.
  - Bytes per list response: 13,020 → 1,329 (brotli, −90%) or 1,500 (gzip).
  - An earlier three-repeat run (`…T061922Z…`) showed the same picture.
- **Validators:** the SPA does not poll. It receives SSE events and refetches after them, when the
  data has changed anyway. ETags would compute the full response to return a 304 of ~1.3 KB
  compressed.

## Decision
- **Static files: DEFAULT.** `npm run build` writes brotli-11 and gzip-9 siblings
  (`frontend/scripts/precompress.mjs`) and moves source maps to `frontend/sourcemaps/`, out of the
  served directory. Hashed assets stay `immutable`; `index.html` stays `no-cache` (revalidated with
  `Last-Modified`).
- **API responses: DEFAULT, switchable** (`http.compression`). tower-http `CompressionLayer`,
  brotli 4 / gzip 6, from 1 KiB. It never compresses:
  - event streams or images;
  - responses that are already encoded (no double compression behind Caddy, which passes them
    through);
  - `Cache-Control: no-store` responses, which carry credentials and must not share a compression
    context with attacker-influenced input (BREACH).
  Turn it off when a proxy in front compresses and CPU on the app tier is the scarce resource.
- **API caching: DEFAULT.** Every API response without its own policy gets `private, no-cache`.
  The user's own browser may keep it but revalidates it, and no CDN or proxy stores it. Handlers
  keep stricter policies (`no-store`).
- **API ETags / 304: REJECTED for now.** Add a weak ETag computed from a cheap version (such as
  `max(updated_at)`) before building the body, on the specific endpoints that polling clients use,
  when such clients exist.

## Consequences
- Tests: `static_assets_use_build_time_compression`,
  `api_responses_are_compressed_by_negotiation_except_secrets_and_streams`,
  `api_responses_are_private_and_handlers_keep_their_own_policy`, and the existing
  `spa_revalidation_304_keeps_app_csp`.
- The release image is about 110 KB larger (the `.br`/`.gz` siblings) and about 600 KB smaller
  (no source maps).
- Error trackers that need source maps upload them from `frontend/sourcemaps/` at build time.
