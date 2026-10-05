# Deployment

One set of binaries (`app-server`, `app-worker`) and one built SPA serve every tier. Behaviour is
configuration (`APP__*` environment variables, see `.env.example`), never a different build.

| tier | for | files | verified by |
|---|---|---|---|
| **Development** | local work | `infra/docker/compose.yaml`, `./dev up` | `./dev check`, E2E |
| **VPS** | one host, low ops cost | `infra/systemd/*.service`, `infra/systemd/Caddyfile` | `infra/verify.sh` (`systemd-analyze verify`, `caddy validate`); **live**: `infra/systemd/live-test.sh` (the release binaries under the real units in a systemd container: start, migrations, non-root, worker, restart, graceful stop, crash restart) |
| **Containers** | one or a few hosts, Docker | `infra/docker/Dockerfile`, `infra/docker/compose.prod.yaml` | **live**: `infra/docker/smoke.sh` (build, start, health, public surface, non-root and read-only, no secrets in the image, restart, graceful SIGTERM) |
| **Kubernetes** | many replicas, autoscaling | `infra/k8s/base` (kustomize) | **static only**: `infra/verify.sh` (kubeconform on files and on the rendered kustomization). Not yet run on a live cluster |
| **Hyperscale edge** | programmable edge on dedicated nodes | `backend/gateway` (Pingora) | `gateway-check`, [benchmarks](../benchmarks/pingora.md) |

Choosing a tier:
- **Start with VPS or Containers.** At the measured rates (thousands of authenticated req/s per
  instance, bounded by PostgreSQL) one well-sized host carries a long way.
- **Move to Kubernetes** when you need several replicas with automated rollout and scaling, or when
  your organisation already runs a cluster.
- **Add the Pingora gateway** only for edge logic that standard proxies make awkward
  ([ADR 0009](../../.ai/knowledge/DECISIONS/0009-pingora-gateway-only-for-edge-needs.md)).

## Invariants for every production tier

- **Configuration:**
  - `APP__ENVIRONMENT=production`. The server refuses to start unless cookies are `Secure`, the
    origins and IdP use https, secrets are set, and bench endpoints are off.
  - `app-server check-config --online` validates configuration and the IdP before rollout.
- **Network exposure:**
  - TLS terminates in front of the app (Caddy, nginx, a cloud load balancer, or an ingress).
  - The **ops port** (`APP__HTTP__OPS_PORT`, 9090 in the image) is never exposed publicly. It serves
    `/metrics` and detailed `/readyz`. The public port answers `/readyz` with a status only.
  - `APP__HTTP__TRUST_FORWARDED_FOR=true` only behind a proxy that overwrites `X-Forwarded-For`.
- **Secrets** come from the environment (`EnvironmentFile`, `env_file`, or a Kubernetes Secret).
  Nothing secret is baked into images or committed.
- **Migrations** run on API start (`APP__DATABASE__MIGRATE_ON_START=true`). sqlx takes a PostgreSQL
  advisory lock, so concurrent replicas are safe.
- **Shutdown:** on SIGTERM the API reports not-ready, drains, then exits within
  `http.shutdown_timeout`. Workers stop claiming jobs and finish or hand back in-flight work. Give
  them a stop timeout of at least 45 s.

## Release build

`backend/Cargo.toml` `[profile.release]` uses thin LTO, codegen-units 1, line-table debug info
and stripped debuginfo. The measured alternatives (no LTO, fat LTO, mimalloc, jemalloc, PGO) are
compared in [docs/benchmarks/release-profile.md](../benchmarks/release-profile.md).

- [VPS (systemd)](vps.md)
- [Containers](containers.md)
- [Kubernetes](kubernetes.md)
