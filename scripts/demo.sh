#!/usr/bin/env bash
# Deterministic demo of the golden path, for screen recording (blueprint only):
#   scripts/demo.sh [DEST] [--pause] [--full]
# command → generated project → setup → running app → login journey (browser, screenshots)
# → live system smoke → production image smoke → (with --full: every validation command) → stop.
#   --pause  wait for Enter between steps (narration)
#   --full   also run ./dev check (≈6–10 min warm)
# DEST defaults to ../demo-product; it must not exist yet. Uses the dev ports, so stop other stacks first.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
dest="../demo-product"; pause=false; full=false
for arg in "$@"; do
  case "$arg" in
    --pause) pause=true ;;
    --full) full=true ;;
    -*) echo "unknown option $arg" >&2; exit 2 ;;
    *) dest="$arg" ;;
  esac
done
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
bold=$'\e[1m'; dim=$'\e[2m'; off=$'\e[0m'
t0=$(date +%s)
step() {
  echo; echo "${bold}▶ $*${off}"
  if $pause; then read -r -p "${dim}  (Enter to continue)${off}" _; fi
}
run() { echo "${dim}\$ $*${off}"; "$@"; }

step "1. Generate a new product (one command)"
cd "$root"
run scripts/create-project "$dest" --name "Demo Product"
cd "$dest"

step "2. The repository explains itself (AI protocol state)"
run python3 tools/ai-check
run sed -n 1,12p README.md

step "3. Set up and start the stack"
run ./dev setup
mkdir -p var
./dev up > var/demo-up.log 2>&1 &
up_pid=$!
trap 'kill -INT $up_pid 2>/dev/null; wait $up_pid 2>/dev/null; ./dev down >/dev/null 2>&1 || true' EXIT
# The generated project has its own ports (infra/dev-ports.env).
read -r api web < <(./dev ports --json --plain | python3 -c 'import json,sys; p=json.load(sys.stdin); print(p["DEV_API_PORT"], p["DEV_WEB_PORT"])')
for _ in $(seq 1 120); do curl -sf "http://localhost:$api/readyz" >/dev/null && curl -sf "http://localhost:$web/" >/dev/null && break; sleep 5; done
echo "  app: http://localhost:$web  (mock sign-in: any email; admin: IdP roles system_admin + Password + TOTP)"
run curl -s "http://localhost:$api/readyz"; echo

step "4. Browser journey per role (screenshots in frontend/test-results/journey/)"
run ./dev test --journey

step "5. One request through every component (login → DB → job → provider → realtime → audit)"
run ./dev test --system

step "6. Production: release image, non-root, read-only, health, no secrets, restart, graceful stop"
run infra/docker/smoke.sh

if $full; then
  step "7. Every canonical validation command"
  run ./dev check
fi

step "Done in $(( $(date +%s) - t0 ))s. Stopping."
