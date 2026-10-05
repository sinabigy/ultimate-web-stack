#!/usr/bin/env bash
# Linux PGO experiment, runs inside rust:1.99 (see pgo_linux.py). Baseline (release, thin LTO)
# vs PGO (instrumented on release-nolto, trained on the bench endpoints, optimised on release).
set -euo pipefail
export RUSTUP_TOOLCHAIN=1.99.0 SQLX_OFFLINE=true CARGO_TERM_COLOR=never
TRIPLE=$(rustc -vV | sed -n 's/host: //p')
rustup component add llvm-tools >/dev/null 2>&1
PROFDATA=$(ls "$(rustc --print sysroot)"/lib/rustlib/*/bin/llvm-profdata | head -1)
if [ ! -x /usr/local/bin/oha ]; then
  a=$(uname -m | sed 's/aarch64/arm64/; s/x86_64/amd64/')
  curl -sSfL -o /usr/local/bin/oha "https://github.com/hatoo/oha/releases/download/v1.16.0/oha-linux-$a" && chmod +x /usr/local/bin/oha
fi
mkdir -p /out/raw
cd /src
t0=$(date +%s); CARGO_TARGET_DIR=/t/base cargo build -q --release -p app-server; echo "base_build_s=$(( $(date +%s) - t0 ))" >> /out/env.txt
rm -rf /pgo && mkdir -p /pgo
CARGO_TARGET_DIR=/t/gen RUSTFLAGS="-Cprofile-generate=/pgo" cargo build -q --profile release-nolto -p app-server --target "$TRIPLE"
start() { # $1=binary
  APP__HTTP__PORT=18080 APP__HTTP__BENCH_ENDPOINTS=true APP__RATE_LIMIT__ENABLED=false APP__JOBS__RUN_IN_PROCESS=false \
  APP__DATABASE__URL="$DB_URL" APP__AUTH__PROVIDER=oidc APP__AUTH__ISSUER_URL=http://127.0.0.1:9 APP__AUTH__CLIENT_ID=x \
  RUST_LOG=warn "$1" >/dev/null 2>&1 &
  echo $! > /tmp/srv.pid
  for _ in $(seq 1 100); do curl -s -o /dev/null http://127.0.0.1:18080/healthz && return 0; sleep 0.1; done; return 1
}
stop() { kill -TERM "$(cat /tmp/srv.pid)"; wait "$(cat /tmp/srv.pid)" 2>/dev/null || true; }
# Training: the same endpoints that are measured.
start "/t/gen/$TRIPLE/release-nolto/app-server"
for p in plaintext json db cached; do oha -z 10s -c 32 --no-tui "http://127.0.0.1:18080/bench/$p" >/dev/null; done
stop
"$PROFDATA" merge -o /pgo/merged.profdata /pgo/*.profraw
t0=$(date +%s); CARGO_TARGET_DIR=/t/use RUSTFLAGS="-Cprofile-use=/pgo/merged.profdata" \
  cargo build -q --release -p app-server --target "$TRIPLE"; echo "pgo_use_build_s=$(( $(date +%s) - t0 ))" >> /out/env.txt
{ echo "kernel=$(uname -r)"; echo "arch=$(uname -m)"; echo "nproc=$(nproc)"; echo "rustc=$(rustc -V)"; } >> /out/env.txt
measure() { # $1=name $2=binary $3=round
  start "$2"
  oha -z 2s -c 32 --no-tui http://127.0.0.1:18080/bench/plaintext >/dev/null
  for p in plaintext json db cached; do
    oha -z "${DURATION:-8s}" -c 64 --no-tui --output-format json "http://127.0.0.1:18080/bench/$p" > "/out/raw/$1-$p-r$3.json"
  done
  stop
}
for r in 1 2; do
  measure baseline /t/base/release/app-server $r
  measure pgo "/t/use/$TRIPLE/release/app-server" $r
done
echo done
