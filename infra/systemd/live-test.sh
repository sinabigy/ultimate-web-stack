#!/usr/bin/env bash
# Live systemd test (opt-in; needs privileged containers): installs the release binaries exactly as
# docs/deployment/vps.md describes, inside a Debian 12 container running systemd, with PostgreSQL
# next to it, then checks start, health, restart, graceful stop and the worker unit.
#   infra/systemd/live-test.sh            (builds or reuses the project's release image, as infra/docker/smoke.sh names it)
set -euo pipefail
[ -n "${TRACE:-}" ] && set -x
root=$(cd "$(dirname "$0")/../.." && pwd)
slug=$(sed -n 's/^name: \${COMPOSE_PROJECT_NAME:-\(.*\)}$/\1/p' "$root/infra/docker/compose.yaml")
image=${APP_IMAGE:-${slug}-release:smoke}
net=$slug-sd-net; pg=$slug-sd-pg; host=$slug-sd-host; work="$root/var/systemd-live"
cleanup() { docker rm -f "$host" "$pg" >/dev/null 2>&1 || true; docker network rm "$net" >/dev/null 2>&1 || true; }
trap cleanup EXIT
cleanup

docker image inspect "$image" >/dev/null 2>&1 \
  || DOCKER_BUILDKIT=1 docker build -q -f "$root/infra/docker/Dockerfile" -t "$image" "$root" >/dev/null
rm -rf "$work" && mkdir -p "$work"
cid=$(docker create "$image"); docker cp "$cid:/app" "$work/app" >/dev/null; docker rm "$cid" >/dev/null

# A Debian 12 host with systemd as PID 1 (built locally: works on amd64 and arm64).
sdimage=app-systemd-debian:12
docker image inspect "$sdimage" >/dev/null 2>&1 || docker build -q -t "$sdimage" - >/dev/null <<'DOCKERFILE'
FROM debian:12
RUN apt-get update && apt-get install -y --no-install-recommends systemd systemd-sysv ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && rm -f /lib/systemd/system/multi-user.target.wants/* /etc/systemd/system/*.wants/* \
      /lib/systemd/system/sockets.target.wants/*udev* /lib/systemd/system/sysinit.target.wants/systemd-tmpfiles-setup*
STOPSIGNAL SIGRTMIN+3
CMD ["/lib/systemd/systemd"]
DOCKERFILE
docker network create "$net" >/dev/null
pw="sd-$(date +%s)"
docker run -d --name "$pg" --network "$net" -e POSTGRES_USER=app -e POSTGRES_DB=app -e POSTGRES_PASSWORD="$pw" \
  postgres:18-alpine >/dev/null
docker run -d --name "$host" --network "$net" --privileged --cgroupns=host \
  -v /sys/fs/cgroup:/sys/fs/cgroup:rw --tmpfs /run --tmpfs /run/lock "$sdimage" >/dev/null
for _ in $(seq 1 30); do docker exec "$pg" pg_isready -U app -d app >/dev/null 2>&1 && break; sleep 1; done
for _ in $(seq 1 30); do state=$(docker exec "$host" systemctl is-system-running 2>/dev/null || true); grep -Eq 'running|degraded' <<<"$state" && break; sleep 1; done

# Install as docs/deployment/vps.md (steps 2–3).
docker cp "$work/app" "$host:/tmp/app" >/dev/null
docker cp "$root/infra/systemd/app-server.service" "$host:/etc/systemd/system/app-server.service"
docker cp "$root/infra/systemd/app-worker.service" "$host:/etc/systemd/system/app-worker.service"
docker exec -i "$host" bash -s <<SH
set -e
useradd --system --home /opt/app --shell /usr/sbin/nologin app
install -d -o root -g root /opt/app/bin /opt/app/web /opt/app/config /etc/app
install -m 0755 /tmp/app/app-server /tmp/app/app-worker /opt/app/bin/
cp -r /tmp/app/web/* /opt/app/web/ && cp /tmp/app/config/app.toml /opt/app/config/
install -m 0640 -o root -g app /dev/null /etc/app/app.env
cat > /etc/app/app.env <<ENV
APP__ENVIRONMENT=production
APP__DATABASE__URL=postgres://app:$pw@$pg:5432/app
APP__DATABASE__MIGRATE_ON_START=true
APP__JOBS__RUN_IN_PROCESS=false
APP__AUTH__PROVIDER=oidc
APP__AUTH__ISSUER_URL=https://id.example.test
APP__AUTH__CLIENT_ID=app-web
APP__AUTH__PUBLIC_ORIGIN=https://app.example.test
APP__AUTH__REDIRECT_URL=https://app.example.test/auth/callback
APP__AUTH__POST_LOGOUT_REDIRECT_URL=https://app.example.test/login
APP__AUTH__COOKIE_SECURE=true
APP__AUTH__TOKEN_ENCRYPTION_KEY=$(head -c 32 /dev/urandom | base64)
APP__AUTH__API_KEY_PEPPER=$(head -c 32 /dev/urandom | base64)
ENV
systemctl daemon-reload
SH

ok() { echo "  PASS  $*"; }
fail() { echo "  FAIL  $*"; docker exec "$host" journalctl -u app-server -u app-worker --no-pager -n 40 || true; exit 1; }
health() { docker exec "$host" /opt/app/bin/app-server healthcheck >/dev/null 2>&1; }
wait_health() { for _ in $(seq 1 40); do health && return 0; sleep 1; done; return 1; }

docker exec "$host" systemctl start app-server || fail "app-server failed to start"
wait_health && ok "app-server active and healthy (ExecStartPre check-config passed, migrations applied)" || fail "app-server not healthy"
[ "$(docker exec "$host" stat -c %U /proc/"$(docker exec "$host" systemctl show -p MainPID --value app-server)")" = app ] \
  && ok "runs as the unprivileged 'app' user" || fail "not running as app"
docker exec "$host" systemctl start app-worker && sleep 3
[ "$(docker exec "$host" systemctl is-active app-worker)" = active ] && ok "app-worker active" || fail "app-worker"
docker exec "$host" systemctl restart app-server && wait_health && ok "restart: healthy again" || fail "restart"
docker exec "$host" systemctl stop app-server
journal=$(docker exec "$host" journalctl -u app-server --no-pager)
grep -q "server stopped cleanly" <<<"$journal" \
  && ok "stop: SIGTERM drained, 'server stopped cleanly'" || fail "stop not graceful"
[ "$(docker exec "$host" systemctl show -p Result --value app-server)" = success ] && ok "stop result: success" || fail "stop result"
docker exec "$host" systemctl kill -s SIGKILL app-worker; sleep 4
[ "$(docker exec "$host" systemctl is-active app-worker)" = active ] && ok "worker killed with SIGKILL: restarted by systemd (Restart=on-failure)" || fail "worker not restarted"
echo "systemd live test: OK"
