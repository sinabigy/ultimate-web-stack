#!/usr/bin/env bash
# Runs inside the Linux benchmark container (see run.py). Builds the runtime variants, then
# measures each with oha. Raw oha JSON goes to /out/raw; environment facts to /out/env.txt.
set -euo pipefail
export RUSTUP_TOOLCHAIN=1.99.0
DURATION=${DURATION:-8s}
CONCS=${CONCS:-"64 256"}
THREADS_LIST=${THREADS_LIST:-"1 2"}
mkdir -p /out/raw
if [ ! -x /usr/local/bin/oha ]; then
  arch=$(uname -m); case "$arch" in aarch64) a=arm64;; x86_64) a=amd64;; *) echo "unsupported $arch"; exit 1;; esac
  curl -sSfL -o /usr/local/bin/oha "https://github.com/hatoo/oha/releases/download/v1.16.0/oha-linux-$a"
  chmod +x /usr/local/bin/oha
fi
cd /src
cargo build --release -q
{
  echo "kernel=$(uname -r)"; echo "arch=$(uname -m)"; echo "nproc=$(nproc)"
  echo "cpu=$(grep -m1 -E 'model name|CPU part' /proc/cpuinfo | cut -d: -f2 | xargs)"
  echo "rustc=$(rustc -V)"; echo "oha=$(oha --version)"
  echo "io_uring_disabled=$(cat /proc/sys/kernel/io_uring_disabled 2>/dev/null || echo n/a)"
} > /out/env.txt
run_variant() { # name env... -- binary
  local name=$1; shift
  for t in $THREADS_LIST; do
    env THREADS=$t ADDR=127.0.0.1:9000 "$@" &
    local pid=$!
    for _ in $(seq 1 50); do curl -s -o /dev/null http://127.0.0.1:9000/ && break; sleep 0.1; done
    oha -z 2s -c 32 --no-tui http://127.0.0.1:9000/ >/dev/null 2>&1 || true
    for c in $CONCS; do
      oha -z "$DURATION" -c "$c" --no-tui --output-format json http://127.0.0.1:9000/ > "/out/raw/$name-t$t-c$c.json" || echo "{}" > "/out/raw/$name-t$t-c$c.json"
    done
    kill "$pid"; wait "$pid" 2>/dev/null || true
    sleep 0.5
  done
}
B=/src/target/release
run_variant tokio_mt "$B/tokio_mt"
run_variant tokio_tpc "$B/tokio_tpc"
run_variant monoio_epoll env DRIVER=epoll "$B/monoio_srv"
run_variant monoio_uring env DRIVER=uring "$B/monoio_srv"
run_variant axum_ref "$B/axum_ref"
echo done
