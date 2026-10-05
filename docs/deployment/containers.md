# Container tier

## Image

`infra/docker/Dockerfile` is a multi-stage build:

```sh
docker build -f infra/docker/Dockerfile -t registry.example.com/app:$(git rev-parse --short HEAD) .
# optional allocator: --build-arg CARGO_FEATURES=alloc-mimalloc (see docs/benchmarks/release-profile.md)
```

| property | value |
|---|---|
| base | `gcr.io/distroless/cc-debian12:nonroot` (no shell, no package manager) |
| contents | `/app/app-server`, `/app/app-worker`, `/app/web` (built SPA) |
| user | `nonroot` (65532) |
| ports | 8080 public; 9090 internal ops (never publish) |
| health | `HEALTHCHECK /app/app-server healthcheck` (calls the ops port's `/readyz`) |
| size | about 111 MB (measured 2026-10-04) |

Build caches: the Cargo registry and target directory, and npm, use BuildKit cache mounts.

## Compose (single host)

`infra/docker/compose.prod.yaml` runs `postgres`, `api` and `worker`:
- **Startup**: the worker waits for the API to become healthy, because the API applies
  migrations first.
- **Hardening**: containers run read-only with a tmpfs `/tmp`, `cap_drop: [ALL]` and
  `no-new-privileges`.
- **Secrets**: the file refuses to start without `APP_ENV_FILE` and `POSTGRES_PASSWORD`.
- **Network**: the API port binds to `127.0.0.1` by default. Put a TLS proxy (Caddy or nginx) in
  front, or set `APP_BIND`.

```sh
APP_ENV_FILE=/etc/app/app.env POSTGRES_PASSWORD=... APP_IMAGE=registry.example.com/app:abc123 \
  docker compose -f infra/docker/compose.prod.yaml up -d
```

## Smoke test

`infra/docker/smoke.sh` is the release gate:
1. builds the image;
2. starts the production stack with throwaway secrets;
3. requires both health checks to pass;
4. checks the public surface: SPA served, `/metrics` 404, `/readyz` status-only;
5. checks the container runs non-root with a read-only filesystem;
6. tears everything down.

On 2026-10-04 it ended with `release smoke: OK (public /readyz={"status":"degraded"})`. The status
is degraded only because the throwaway IdP host does not resolve, and that check is non-critical.
