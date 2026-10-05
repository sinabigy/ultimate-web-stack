# VPS tier (systemd)

A single Linux host running PostgreSQL, `app-server` (API plus static SPA) and, optionally,
`app-worker`, behind Caddy for automatic HTTPS.

## Install

```sh
# 1. Build on a matching architecture (or copy from the release image: docker cp)
cd backend && cargo build --release -p app-server -p app-worker
cd ../frontend && npm ci && npm run build

# 2. Lay out /opt/app
sudo useradd --system --home /opt/app --shell /usr/sbin/nologin app
sudo install -d -o root -g root /opt/app/bin /opt/app/web /etc/app
sudo install -m 0755 backend/target/release/app-server backend/target/release/app-worker /opt/app/bin/
sudo cp -r frontend/dist/* /opt/app/web/
sudo install -d /opt/app/config && sudo cp backend/config/app.toml /opt/app/config/   # non-secret settings

# 3. Configuration (secrets: readable by the service user only)
sudo install -m 0640 -o root -g app /dev/null /etc/app/app.env   # then edit; see .env.example
#   APP__ENVIRONMENT=production, APP__DATABASE__URL=..., APP__AUTH__* (https), secrets

# 4. Units and TLS
sudo cp infra/systemd/app-server.service infra/systemd/app-worker.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo -u app /opt/app/bin/app-server check-config --online   # needs the env file exported
sudo systemctl enable --now app-server           # and app-worker (set APP__JOBS__RUN_IN_PROCESS=false)
sudo cp infra/systemd/Caddyfile /etc/caddy/Caddyfile && sudo systemctl reload caddy
```

## What the units do

| setting | why |
|---|---|
| `ExecStartPre=app-server check-config` | refuses to start with an invalid production configuration |
| `APP__HTTP__HOST=127.0.0.1`, ops port 9090 | only Caddy reaches the app; metrics and details stay on loopback |
| `KillSignal=SIGTERM`, `TimeoutStopSec=45` | graceful drain (readiness off, connections finish, jobs hand back) |
| `ProtectSystem=strict`, `NoNewPrivileges`, empty `CapabilityBoundingSet`, `SystemCallFilter=@system-service`, `MemoryDenyWriteExecute`, … | `systemd-analyze security` exposure **1.6 (OK)**; the app needs no writable paths |
| `LimitNOFILE=1048576` | many keep-alive connections |

## Operations

- **Logs**: `journalctl -u app-server -o cat`. The output is JSON; ship it with Alloy, Vector or
  Promtail if you want Loki.
- **Metrics**: scrape `127.0.0.1:9090/metrics` (API) and `127.0.0.1:9091/metrics` (worker).
- **Upgrades**: replace the binaries, run `systemctl restart app-server app-worker`, and check
  readiness on the internal port with `/opt/app/bin/app-server healthcheck`.
- **Backups**: `pg_dump` or `pgBackRest`. PostgreSQL is the only state, apart from the optional
  ClickHouse and NATS.

Verification (no VPS needed): `infra/verify.sh` runs `systemd-analyze verify` on both units and
`caddy validate` on the Caddyfile inside containers.
