# Optional modules: enabling one after generation

`scripts/create-project` selects modules at generation time. A module that was not selected
leaves no trace that runs or fails:
- not compiled: it is a cargo feature outside `default`;
- not enabled in `backend/config/app.toml`;
- not started by `./dev up`, and no service in CI;
- no validation command in `.ai/config/project.json`.

What stays is inert: the source behind its cargo feature, and its compose profile in
`infra/docker/compose.yaml`. That is all you need to switch it on later. Each module below takes
the same five steps, and the result is identical to selecting it at generation:
1. cargo `default` features;
2. `app.toml`;
3. `project.json` (the architecture module and the validation command);
4. CI;
5. verify.

The verify step is always the same:

```sh
./dev up --no-app && ./dev check && ./dev up    # then, in another terminal:
./dev test --system                              # the module's check turns from SKIP to PASS
```

At startup the server logs one `configuration` line, which shows what it actually runs with:
`authorization=… cache=… messaging_nats=… analytics_clickhouse=…`.

## Redis cache and shared rate limits (`cache`)
When: more than one API instance. On one instance the in-process cache is 2.2× faster (see
`docs/benchmarks/SUMMARY.md`).
1. `backend/apps/server/Cargo.toml`: add `"redis"` to `default`.
2. `backend/config/app.toml`:
   ```toml
   [cache]
   backend = "redis"
   redis_url = "redis://127.0.0.1:56379"   # development; production: APP__CACHE__REDIS_URL
   ```
   Optionally also set `[rate_limit] backend = "redis"`, so limits are shared across instances.
3. `.ai/config/project.json`:
   - set `architecture.modules.cache.adapter` to `"redis"` (this makes `./dev up` start Redis);
   - add this command:
     ```json
     {"id": "rust-test-cache", "stage": "integration", "cmd": "TEST_REDIS_URLS=redis://127.0.0.1:56379 cargo test -p app-cache --features redis 2>&1", "cwd": "backend", "requires": ["cargo"], "source": "user", "confirmed": true, "timeout_s": 1200}
     ```
4. `.github/workflows/ci.yml`, backend job:
   - add a `redis` service (`redis:8-alpine`, port `56379:6379`, health check `redis-cli ping`);
   - add `--only rust-test-cache`.
5. Production: run with `COMPOSE_PROFILES=cache` (the `redis` service in
   `infra/docker/compose.prod.yaml`), or point `APP_REDIS_URL` at a managed Redis.
   `infra/docker/smoke.sh` does this automatically when `app.toml` selects Redis.

## NATS realtime bus and JetStream queue (`messaging_nats`)
When: high job volume (about 10× the PostgreSQL queue, measured) or consumers in other services.
The example runs stay on the PostgreSQL queue, because their enqueue is transactional with
business rows. See `docs/architecture/messaging.md`.
1. `default` features: add `"nats"` in `backend/apps/server/Cargo.toml`,
   `backend/apps/worker/Cargo.toml` and `backend/apps/bench/Cargo.toml`.
2. `backend/config/app.toml`: `[messaging] enabled = true`. The default `nats_url` is
   `nats://127.0.0.1:54222`; production uses `APP__MESSAGING__NATS_URL`.
3. `.ai/config/project.json`:
   - set `architecture.modules.messaging_nats.enabled` to `true`;
   - add this command:
     ```json
     {"id": "rust-test-nats", "stage": "integration", "cmd": "TEST_NATS_URL=${TEST_NATS_URL:-nats://127.0.0.1:54222} cargo test -p app-messaging --features nats 2>&1", "cwd": "backend", "requires": ["cargo"], "source": "user", "confirmed": true, "timeout_s": 1200}
     ```
4. CI backend job:
   - add a step before validation: `docker run -d --name nats -p 54222:4222 nats:2.14.7-alpine --jetstream`;
   - add `--only rust-test-nats`.
5. Production: add a NATS service with JetStream, or a managed NATS. If NATS is unreachable at
   startup, the server falls back to the PostgreSQL event bus and logs it.

## ClickHouse event analytics (`analytics_clickhouse`)
When: aggregates across tenants or very large event volumes (about 24× faster cross-tenant
aggregates, measured). Per-tenant dashboards are fine on PostgreSQL. See
`docs/architecture/analytics.md`.
1. `default` features: add `"clickhouse"` in the server, worker and bench `Cargo.toml` files.
2. `backend/config/app.toml`: `[analytics] enabled = true`. Development defaults:
   `http://127.0.0.1:58123`, user `app`. Production uses `APP__ANALYTICS__*`.
3. `.ai/config/project.json`:
   - set `architecture.modules.analytics_clickhouse.enabled` to `true`;
   - add this command:
     ```json
     {"id": "rust-test-clickhouse", "stage": "integration", "cmd": "export TEST_CLICKHOUSE_URL=${TEST_CLICKHOUSE_URL:-http://127.0.0.1:58123}; cargo test -p app-analytics --features clickhouse 2>&1 && cargo test -p app-api --features clickhouse --test analytics 2>&1", "cwd": "backend", "requires": ["cargo"], "source": "user", "confirmed": true, "timeout_s": 1200}
     ```
4. CI backend job:
   - add a `clickhouse` service (`clickhouse/clickhouse-server:26.3.25.2`, port `58123:8123`,
     env `CLICKHOUSE_DB=app`, `CLICKHOUSE_USER=app`, `CLICKHOUSE_PASSWORD=app-dev-only`,
     `CLICKHOUSE_DEFAULT_ACCESS_MANAGEMENT=1`);
   - add `--only rust-test-clickhouse`.
5. The dev VM needs at least 6 GiB with ClickHouse running (`.ai/knowledge/KNOWN_ISSUES.md`).

## Cedar policy engine (`cedar`)
When: authorization rules outgrow role → permission tables (attribute conditions, policies that
non-engineers review). RBAC and Cedar give identical decisions on the shipped policies, and a
differential property test checks this.
1. `backend/apps/server/Cargo.toml`: add `"cedar"` to `default`.
2. `backend/config/app.toml`: `[authorization] engine = "cedar"`. Policies are in
   `authorization.cedar_policy_dir`; see `docs/authorization/README.md`.
3. `.ai/config/project.json`:
   - set `architecture.modules.cedar.enabled` to `true`;
   - add this command:
     ```json
     {"id": "rust-test-cedar", "stage": "unit", "cmd": "cargo test -p app-authz --features cedar 2>&1", "cwd": "backend", "requires": ["cargo"], "source": "user", "confirmed": true, "timeout_s": 1800}
     ```
4. CI: add `--only rust-test-cedar` to the backend validation step.

## Removing a module
Apply the same steps in reverse. `./dev check` and CI then no longer reference it, and
`./dev test --system` reports its check as SKIP.
