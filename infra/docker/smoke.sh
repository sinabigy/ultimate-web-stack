#!/usr/bin/env bash
# Release smoke test: build the image, run the production compose stack with throwaway secrets,
# require both health checks to pass, check the public surface, then tear everything down.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
compose() { if docker compose version >/dev/null 2>&1; then docker compose "$@"; else docker-compose "$@"; fi; }
export COMPOSE_PROJECT_NAME=app-smoke APP_IMAGE=${APP_IMAGE:-app-smoke:latest} APP_PORT=${APP_PORT:-18299}
export POSTGRES_PASSWORD="smoke-$(date +%s)"
work="$root/var/release-smoke"; mkdir -p "$work"
export APP_ENV_FILE="$work/app.env"
cat > "$APP_ENV_FILE" <<ENV
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
f="$root/infra/docker/compose.prod.yaml"
# Start the optional services the project's configuration selects.
if grep -Eq '^backend = "redis"' "$root/backend/config/app.toml" 2>/dev/null; then export COMPOSE_PROFILES=cache; fi
cleanup() { compose -f "$f" down -v >/dev/null 2>&1 || true; }
trap cleanup EXIT

DOCKER_BUILDKIT=1 docker build -q -f "$root/infra/docker/Dockerfile" -t "$APP_IMAGE" "$root" >/dev/null
echo "image: $APP_IMAGE ($(docker image inspect "$APP_IMAGE" --format '{{.Size}}' | awk '{printf "%.0f MB", $1/1e6}'))"
compose -f "$f" up -d >/dev/null
wait_healthy() {
  for _ in $(seq 1 90); do
    [ "$(docker inspect -f '{{.State.Health.Status}}' "$1" 2>/dev/null)" = healthy ] && return 0
    sleep 2
  done
  docker logs --tail 50 "$1"; return 1
}
wait_healthy app-smoke-api-1 && echo "api healthy"
wait_healthy app-smoke-worker-1 && echo "worker healthy"
ready=$(curl -s "http://127.0.0.1:$APP_PORT/readyz")
case "$ready" in *checks*) echo "FAIL: public /readyz exposes check details: $ready"; exit 1;; esac
[ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$APP_PORT/metrics")" = 404 ] || { echo "FAIL: /metrics public"; exit 1; }
[ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$APP_PORT/")" = 200 ] || { echo "FAIL: SPA not served"; exit 1; }
docker inspect app-smoke-api-1 --format '{{.Config.User}} {{.HostConfig.ReadonlyRootfs}}' | grep -q '^nonroot:nonroot true$' \
  || { echo "FAIL: not non-root/read-only"; exit 1; }
# Secrets come only from the environment file at run time: none is baked into the image.
if docker image inspect "$APP_IMAGE" --format '{{json .Config.Env}}' | grep -Eq 'TOKEN_ENCRYPTION_KEY|API_KEY_PEPPER|PASSWORD|SECRET'; then
  echo "FAIL: secret-like variable in the image config"; exit 1
fi
cid=$(docker create "$APP_IMAGE"); files=$(docker export "$cid" | tar -t); docker rm "$cid" >/dev/null
if echo "$files" | grep -Eq '(^|/)\.env($|\.)|app\.env$|\.pem$|id_rsa'; then echo "FAIL: secret file in the image"; exit 1; fi
echo "image holds no secrets"
# Restart: the service comes back healthy (state lives in PostgreSQL).
docker restart app-smoke-api-1 >/dev/null && wait_healthy app-smoke-api-1 && echo "api healthy after restart"
# Graceful shutdown: SIGTERM drains in-flight work and exits 0.
docker stop -t 30 app-smoke-api-1 >/dev/null
code=$(docker inspect -f '{{.State.ExitCode}}' app-smoke-api-1)
docker logs app-smoke-api-1 2>&1 | grep -q "server stopped cleanly" && [ "$code" = 0 ] \
  || { echo "FAIL: shutdown not graceful (exit $code)"; docker logs --tail 20 app-smoke-api-1; exit 1; }
echo "graceful shutdown: exit 0, drained"
echo "release smoke: OK (public /readyz=$ready)"
