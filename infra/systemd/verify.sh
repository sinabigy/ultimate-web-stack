#!/usr/bin/env bash
# Verify the unit files with systemd-analyze in a Linux container (no systemd host needed).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
docker run --rm -v "$here:/units:ro" debian:bookworm-slim bash -c '
  set -e
  apt-get update -qq >/dev/null && apt-get install -y -qq systemd >/dev/null
  useradd --system app
  mkdir -p /opt/app/bin /etc/app && touch /etc/app/app.env
  printf "#!/bin/sh\n" > /opt/app/bin/app-server && printf "#!/bin/sh\n" > /opt/app/bin/app-worker
  chmod +x /opt/app/bin/*
  cp /units/*.service /etc/systemd/system/
  systemd-analyze verify /etc/systemd/system/app-server.service /etc/systemd/system/app-worker.service
  echo "systemd-analyze verify: OK"
  systemd-analyze security --offline=true /etc/systemd/system/app-server.service 2>/dev/null | tail -1 || true
'
