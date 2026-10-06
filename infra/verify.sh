#!/usr/bin/env bash
# Static verification of every deployment tier (needs Docker; no cluster or systemd host).
#   compose: `docker compose config` for every dev profile and the production file
#   systemd: systemd-analyze verify in a Debian container
#   k8s:     kubeconform -strict on the base and on the rendered kustomization
#   caddy:   caddy validate
#   alerts:  promtool check + rule unit tests
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
compose() { if docker compose version >/dev/null 2>&1; then docker compose "$@"; else docker-compose "$@"; fi; }
fail=0
step() { printf '%-58s' "$1"; shift; if out=$("$@" 2>&1); then echo OK; else echo FAIL; echo "$out" | tail -20; fail=1; fi; }

for p in "" cache dragonfly messaging analytics identity observability; do
  step "compose (dev) profile ${p:-core}" compose -f "$root/infra/docker/compose.yaml" ${p:+--profile "$p"} config -q
done
step "compose (production)" env APP_ENV_FILE=/dev/null POSTGRES_PASSWORD=verify \
  bash -c "$(declare -f compose); compose -f '$root/infra/docker/compose.prod.yaml' config -q"
step "systemd units (systemd-analyze verify)" "$root/infra/systemd/verify.sh"
step "kubernetes base (kubeconform -strict)" docker run --rm -v "$root/infra/k8s:/k8s:ro" ghcr.io/yannh/kubeconform:latest \
  -strict -summary -ignore-filename-pattern 'kustomization.yaml|secret.example.yaml' /k8s/base
step "kubernetes kustomize render + kubeconform" bash -c "docker run --rm -v '$root/infra/k8s:/k8s:ro' registry.k8s.io/kustomize/kustomize:v5.7.1 build /k8s/base 2>/dev/null | docker run --rm -i ghcr.io/yannh/kubeconform:latest -strict -summary -"
step "Caddyfile (caddy validate)" docker run --rm -v "$root/infra/systemd/Caddyfile:/etc/caddy/Caddyfile:ro" caddy:2 \
  caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
obs="$root/infra/docker/observability"
step "alert rules (promtool check rules + test rules)" docker run --rm --entrypoint sh -v "$obs:/r:ro" -w /r \
  prom/prometheus:v3.15.0 -c "promtool check rules alerts.yml && promtool test rules alerts.test.yml && promtool check config --syntax-only prometheus.yml"
exit $fail
