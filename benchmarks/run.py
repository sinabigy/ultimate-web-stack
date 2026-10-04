#!/usr/bin/env python3
"""Benchmark orchestrator. Measures, never estimates: a suite that cannot run is recorded as
skipped with the reason, and no number is ever written that was not observed.

    python3 benchmarks/run.py                      # all suites, full durations
    python3 benchmarks/run.py --smoke              # short run (CI perf-smoke)
    python3 benchmarks/run.py --suite http,auth    # subset
    python3 benchmarks/run.py --label after-lto    # tag the result file

Suites:
  http      bench endpoints via oha (plaintext/json concurrency sweep, db read/write, cached read)
  auth      authenticated requests through the real cookie→session→authorization path
  cache     /bench/cached with the cache backend set to memory, Redis and Dragonfly
  outbound  outbound engine vs a naive client against a simulated provider (app-bench)
  db        PostgreSQL workloads and pool behaviour (app-bench)
  messaging PostgreSQL job queue vs NATS JetStream: publish and drain rates (app-bench queue)
  analytics ClickHouse vs PostgreSQL: sink ingest, tenant and cross-tenant aggregates, storage
  gateway   direct vs through the Pingora gateway (hyperscale profile): added latency, throughput, gateway CPU/RSS

Output: benchmarks/results/<UTC timestamp>-<git sha>[-label].json and benchmarks/results/latest.json.
Then: `python3 benchmarks/compare.py` (regression gates) and `python3 benchmarks/report.py` (docs).
Requires: release builds (done here), PostgreSQL from `./dev up --no-app`, `oha` on PATH;
Redis/Dragonfly only for the cache suite (`./dev up --no-app` with the cache module).
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import platform
import shutil
import socket
import subprocess
import sys
import threading
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BACKEND = ROOT / "backend"
BIN = BACKEND / "target" / "release"
RESULTS = ROOT / "benchmarks" / "results"
PG_ADMIN_URL = os.environ.get("BENCH_PG_ADMIN_URL", "postgres://app:app-dev-only@localhost:55432/app")
BENCH_DB_URL = os.environ.get("BENCH_DATABASE_URL", "postgres://app:app-dev-only@localhost:55432/app_bench")
REDIS_URL = os.environ.get("BENCH_REDIS_URL", "redis://127.0.0.1:56379")
DRAGONFLY_URL = os.environ.get("BENCH_DRAGONFLY_URL", "redis://127.0.0.1:56380")
APP_PORT, IDP_PORT = 18090, 59083
APP = f"http://127.0.0.1:{APP_PORT}"
SUITES = ["http", "auth", "cache", "outbound", "db", "messaging", "analytics", "gateway"]


def log(msg: str) -> None:
    print(f"[bench] {msg}", file=sys.stderr, flush=True)


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    r = subprocess.run(cmd, text=True, capture_output=True, **kw)
    if r.returncode != 0:
        raise RuntimeError(f"{Path(cmd[0]).name} {' '.join(cmd[1:3])} exited {r.returncode}: {r.stderr.strip()[-1500:]}")
    return r


def try_out(cmd: list[str]) -> str | None:
    try:
        return subprocess.run(cmd, text=True, capture_output=True, timeout=10).stdout.strip() or None
    except (OSError, subprocess.TimeoutExpired):
        return None


# ── environment ──────────────────────────────────────────────────────────────────────────────


def environment() -> dict:
    sysname = platform.system()
    if sysname == "Darwin":
        cpu = try_out(["sysctl", "-n", "machdep.cpu.brand_string"])
        mem = int(try_out(["sysctl", "-n", "hw.memsize"]) or 0)
        os_ver = f"macOS {platform.mac_ver()[0]}"
    else:
        cpu = next((ln.split(":", 1)[1].strip() for ln in Path("/proc/cpuinfo").read_text().splitlines()
                    if ln.startswith("model name")), None) if Path("/proc/cpuinfo").exists() else None
        mem = 0
        if Path("/proc/meminfo").exists():
            kb = int(Path("/proc/meminfo").read_text().split()[1])
            mem = kb * 1024
        os_ver = f"{sysname} {platform.release()}"
    dirty = bool(try_out(["git", "-C", str(ROOT), "status", "--porcelain", "--untracked-files=no"]))
    docker = try_out(["docker", "info", "--format", "{{.NCPU}} cpus, {{.MemTotal}} bytes, {{.OperatingSystem}}, {{.Architecture}}"])
    return {
        "host": socket.gethostname().split(".")[0],
        "os": os_ver,
        "arch": platform.machine(),
        "cpu": cpu,
        "logical_cores": os.cpu_count(),
        "memory_gb": round(mem / 2**30, 1) if mem else None,
        "rustc": try_out(["rustc", "-V"]),
        "oha": try_out(["oha", "--version"]),
        "git_sha": try_out(["git", "-C", str(ROOT), "rev-parse", "--short=12", "HEAD"]),
        "git_dirty": dirty,
        "build_profile": "release (lto=thin, codegen-units=1)",
        "container_runtime": docker,
        "notes": "Load generator and server share the host; services (PostgreSQL/Redis/Dragonfly) run in the container VM.",
    }


# ── processes ────────────────────────────────────────────────────────────────────────────────


def wait_http(url: str, timeout: float) -> bool:
    end = time.time() + timeout
    while time.time() < end:
        try:
            with urllib.request.urlopen(url, timeout=1) as r:
                if r.status < 500:
                    return True
        except Exception:  # noqa: BLE001 - polling
            time.sleep(0.2)
    return False


class Stack:
    """mock-oidc + app-server with bench endpoints, isolated ports and database."""

    def __init__(self, extra_env: dict[str, str] | None = None):
        self.extra = extra_env or {}
        self.procs: list[subprocess.Popen] = []
        self.server: subprocess.Popen | None = None

    def __enter__(self) -> "Stack":
        env = dict(os.environ)
        env.update({
            "RUST_LOG": "warn",
            "MOCK_OIDC_PORT": str(IDP_PORT),
            "MOCK_OIDC_REDIRECT_URIS": f"{APP}/auth/callback",
            "MOCK_OIDC_POST_LOGOUT_URIS": f"{APP}/login",
        })
        self.procs.append(subprocess.Popen([str(BIN / "mock-oidc")], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        env.update({
            "APP__ENVIRONMENT": "test",
            "APP__HTTP__PORT": str(APP_PORT),
            "APP__HTTP__BENCH_ENDPOINTS": "true",
            "APP__DATABASE__URL": BENCH_DB_URL,
            "APP__DATABASE__MIGRATE_ON_START": "true",
            "APP__RATE_LIMIT__ENABLED": "false",
            "APP__JOBS__RUN_IN_PROCESS": "false",
            "APP__LOG__FORMAT": "json",
            "APP__AUTH__PROVIDER": "oidc",
            "APP__AUTH__ISSUER_URL": f"http://127.0.0.1:{IDP_PORT}",
            "APP__AUTH__CLIENT_ID": "app-web",
            "APP__AUTH__PUBLIC_ORIGIN": APP,
            "APP__AUTH__REDIRECT_URL": f"{APP}/auth/callback",
            "APP__AUTH__POST_LOGOUT_REDIRECT_URL": f"{APP}/login",
        })
        env.update(EXTRA_ENV)
        env.update(self.extra)
        self.server = subprocess.Popen([str(BIN / "app-server")], cwd=BACKEND, env=env,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        self.procs.append(self.server)
        if not wait_http(f"{APP}/readyz", 60):
            err = self.server.stderr.read() if self.server.poll() is not None and self.server.stderr else ""
            self.__exit__(None, None, None)
            raise RuntimeError(f"app-server did not become ready: {err[-2000:]}")
        return self

    def __exit__(self, *_exc) -> None:
        for p in self.procs:
            if p.poll() is None:
                p.terminate()
        for p in self.procs:
            try:
                p.wait(timeout=20)
            except subprocess.TimeoutExpired:
                p.kill()


class Sampler(threading.Thread):
    """Samples a process's RSS and CPU% with `ps` while a load test runs."""

    def __init__(self, pid: int):
        super().__init__(daemon=True)
        self.pid, self.stop, self.rss_kb, self.cpu = pid, threading.Event(), [], []

    def run(self) -> None:
        while not self.stop.is_set():
            out = try_out(["ps", "-o", "rss=,%cpu=", "-p", str(self.pid)])
            if out:
                rss, cpu = out.split()[:2]
                self.rss_kb.append(int(rss))
                self.cpu.append(float(cpu))
            self.stop.wait(0.5)

    def result(self) -> dict:
        # Skip the first sample (ps %cpu is averaged since the previous sample).
        cpu = self.cpu[1:] or self.cpu
        return {
            "server_rss_mb_max": round(max(self.rss_kb) / 1024, 1) if self.rss_kb else None,
            "server_cpu_pct_avg": round(sum(cpu) / len(cpu), 1) if cpu else None,
        }


# ── load generation ──────────────────────────────────────────────────────────────────────────


REPEAT = 1  # set from --repeat; measured points run this many times and report the median
EXTRA_ENV: dict[str, str] = {}  # set from --env; applied to every app-server started


def oha(url: str, duration: str, conc: int, headers: dict[str, str] | None = None, pid: int | None = None) -> dict:
    """Median (by success req/s) of REPEAT runs, with the spread recorded. Single 10 s samples on
    a shared host varied by ±20% between identical runs, so one sample cannot gate regressions."""
    if pid is None or REPEAT <= 1:
        return oha_once(url, duration, conc, headers, pid)
    runs = sorted((oha_once(url, duration, conc, headers, pid) for _ in range(REPEAT)), key=lambda r: r["success_rps"])
    med = dict(runs[len(runs) // 2])
    samples = [r["success_rps"] for r in runs]
    med["repeats"] = len(runs)
    med["success_rps_samples"] = samples
    med["success_rps_spread"] = round((samples[-1] - samples[0]) / med["success_rps"], 4) if med["success_rps"] else None
    return med


def oha_once(url: str, duration: str, conc: int, headers: dict[str, str] | None = None, pid: int | None = None) -> dict:
    cmd = ["oha", "-z", duration, "-c", str(conc), "--no-tui", "--output-format", "json", url]
    for k, v in (headers or {}).items():
        cmd[1:1] = ["-H", f"{k}: {v}"]
    sampler = Sampler(pid) if pid else None
    if sampler:
        sampler.start()
    try:
        raw = json.loads(run(cmd).stdout)
    finally:
        if sampler:
            sampler.stop.set()
            sampler.join()
    s, lat = raw["summary"], raw.get("latencyPercentiles", {})
    codes = {str(k): v for k, v in (raw.get("statusCodeDistribution") or {}).items()}
    # Requests still in flight when the duration ends are cancelled by oha ("aborted due to
    # deadline"); they are an artefact of stopping, not failures, so they are excluded.
    errors = {k: v for k, v in (raw.get("errorDistribution") or {}).items() if "deadline" not in k}
    total = sum(codes.values()) + sum(errors.values())
    ok = sum(v for k, v in codes.items() if k.startswith("2"))
    elapsed = s.get("total") or 1
    ms = lambda k: round(lat[k] * 1000, 3) if lat.get(k) is not None else None  # noqa: E731
    r = {
        "url_path": url.replace(APP, ""),
        "concurrency": conc,
        "duration_s": round(elapsed, 2),
        "requests": total,
        "rps": round(total / elapsed, 1),
        "success_rps": round(ok / elapsed, 1),
        "success_rate": round(ok / total, 5) if total else 0.0,
        "status_codes": codes,
        "errors": errors,
        "p50_ms": ms("p50"), "p95_ms": ms("p95"), "p99_ms": ms("p99"), "p999_ms": ms("p99.9"),
    }
    if sampler:
        r.update(sampler.result())
        cpu = r.get("server_cpu_pct_avg")
        if cpu:
            r["success_rps_per_server_core"] = round(r["success_rps"] / (cpu / 100), 1)
        if r.get("server_rss_mb_max"):
            r["success_rps_per_gb_rss"] = round(r["success_rps"] / (r["server_rss_mb_max"] / 1024), 1)
    return r


# ── suites ───────────────────────────────────────────────────────────────────────────────────


def suite_http(smoke: bool) -> dict:
    d = "3s" if smoke else "6s"
    sweep = [64] if smoke else [16, 64, 256]
    out: dict = {"duration_per_point": d}
    with Stack() as st:
        pid = st.server.pid
        oha(f"{APP}/bench/plaintext", "2s", 32)  # warm-up (connections, allocator, page cache)
        for name in ["plaintext", "json"]:
            out[name] = [oha(f"{APP}/bench/{name}", d, c, pid=pid) for c in sweep]
        for name in ["db", "updates", "cached"]:
            out[name] = oha(f"{APP}/bench/{name}", d, 64, pid=pid)
        out["healthz"] = oha(f"{APP}/healthz", d, 64, pid=pid)
    return out


def seed_http() -> dict:
    return json.loads(run([str(BIN / "app-bench"), "seed-http"], env={**os.environ, "BENCH_DATABASE_URL": BENCH_DB_URL}).stdout)


def suite_auth(smoke: bool) -> dict:
    d = "3s" if smoke else "6s"
    fx = seed_http()
    cookie = {"Cookie": f"app_session={fx['cookie_value']}"}
    out: dict = {"duration_per_point": d, "fixture": {"org_slug": fx["org_slug"], "runs_in_org": 1000}}
    with Stack() as st:
        pid = st.server.pid
        oha(f"{APP}/api/v1/session", "2s", 32, cookie)
        # `db_queries` = sequential PostgreSQL round trips per request (read from the handlers).
        # Session lookup (cookie → hashed token → session+user row) + user row.
        out["profile"] = {"db_queries": 2, **oha(f"{APP}/api/v1/account/profile", d, 64, cookie, pid)}
        # Session lookup + organisations list + unread count (what the SPA loads first).
        out["session"] = {"db_queries": 3, **oha(f"{APP}/api/v1/session", d, 64, cookie, pid)}
        # Session + membership facts (role → permission check) + tenant-scoped indexed page of 20.
        out["org_runs_page"] = {"db_queries": 3, **oha(f"{APP}/api/v1/orgs/{fx['org_slug']}/runs?limit=20", d, 64, cookie, pid)}
        # Rejection cost: no credentials → 401 problem+json.
        out["unauthenticated_401"] = {"db_queries": 0, **oha(f"{APP}/api/v1/dashboard", d, 64, None, pid)}
        # Cross-tenant probe → 404 (membership check fails; audited).
        out["cross_tenant_404"] = oha(f"{APP}/api/v1/orgs/not-a-member-org/runs", "2s" if smoke else "4s", 16, cookie, pid)
    return out


def redis_ping(url: str) -> bool:
    host, port = url.split("//", 1)[1].split(":")
    try:
        with socket.create_connection((host, int(port)), timeout=1) as s:
            s.sendall(b"PING\r\n")
            return s.recv(16).startswith(b"+PONG")
    except OSError:
        return False


def suite_cache(smoke: bool) -> dict:
    d = "3s" if smoke else "6s"
    out: dict = {"duration_per_point": d, "endpoint": "/bench/cached (hot set of 100 keys, read-through to PostgreSQL)"}
    backends = {"memory": None, "redis": REDIS_URL, "dragonfly": DRAGONFLY_URL}
    for name, url in backends.items():
        if url and not redis_ping(url):
            out[name] = {"skipped": f"{url} not reachable (start with ./dev up --no-app)"}
            continue
        env = {"APP__CACHE__BACKEND": "memory" if url is None else "redis"}
        if url:
            env["APP__CACHE__REDIS_URL"] = url
        with Stack(env) as st:
            oha(f"{APP}/bench/cached", "2s", 32)
            out[name] = oha(f"{APP}/bench/cached", d, 64, pid=st.server.pid)
    return out


GATEWAY_PORT = 18100
NGINX_PORT = 18101

NGINX_CONF = """
worker_processes {workers};
pid {dir}/nginx.pid;
error_log {dir}/error.log warn;
events {{ worker_connections 4096; }}
http {{
    access_log off;
    upstream app {{ server 127.0.0.1:{app_port}; keepalive 256; }}
    server {{
        listen 127.0.0.1:{port} reuseport;
        location / {{
            proxy_pass http://app;
            proxy_http_version 1.1;
            proxy_set_header Connection "";
            proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        }}
    }}
}}
"""


def start_nginx(workers: str) -> subprocess.Popen | None:
    """Baseline proxy with the same worker count and upstream keep-alive, when nginx exists."""
    exe = shutil.which("nginx") or ("/opt/homebrew/bin/nginx" if Path("/opt/homebrew/bin/nginx").exists() else None)
    if not exe:
        return None
    d = Path(os.environ.get("TMPDIR", "/tmp")) / f"bench-nginx-{os.getpid()}"
    d.mkdir(parents=True, exist_ok=True)
    (d / "nginx.conf").write_text(NGINX_CONF.format(workers=workers, dir=d, app_port=APP_PORT, port=NGINX_PORT))
    return subprocess.Popen([exe, "-p", str(d), "-c", str(d / "nginx.conf"), "-g", "daemon off;"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def suite_gateway(smoke: bool) -> dict:
    gw_dir = BACKEND / "gateway"
    gw_bin = gw_dir / "target" / "release" / "app-gateway"
    log("building gateway (separate workspace; needs cmake)…")
    run(["cargo", "build", "--release", "-q"], cwd=gw_dir)
    d = "3s" if smoke else "6s"
    conc = [64] if smoke else [64, 256]
    gw_url = f"http://127.0.0.1:{GATEWAY_PORT}"
    threads = "4"
    out: dict = {"duration_per_point": d, "gateway_threads": int(threads), "points": []}
    with Stack() as st:
        env = {**os.environ, "GATEWAY_LISTEN": f"127.0.0.1:{GATEWAY_PORT}", "GATEWAY_UPSTREAMS": f"127.0.0.1:{APP_PORT}",
               "GATEWAY_THREADS": threads}
        gw = subprocess.Popen([str(gw_bin)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        ngx = start_nginx(threads)
        ngx_url = f"http://127.0.0.1:{NGINX_PORT}"
        out["baseline_proxy"] = "nginx" if ngx else None
        try:
            if ngx and not wait_http(f"{ngx_url}/healthz", 15):
                ngx.terminate()
                ngx = None
                out["baseline_proxy"] = None
            if not wait_http(f"{gw_url}/healthz", 30):
                raise RuntimeError("gateway did not answer")
            oha(f"{APP}/bench/plaintext", "2s", 32)
            oha(f"{gw_url}/bench/plaintext", "2s", 32)
            for path in ["/bench/plaintext", "/bench/json", "/bench/db"]:
                for c in conc:
                    direct = oha(f"{APP}{path}", d, c, pid=st.server.pid)
                    sampler = Sampler(gw.pid)
                    sampler.start()
                    try:
                        proxied = oha(f"{gw_url}{path}", d, c, pid=st.server.pid)
                    finally:
                        sampler.stop.set()
                        sampler.join()
                    g = sampler.result()
                    proxied["gateway_cpu_pct_avg"] = g["server_cpu_pct_avg"]
                    proxied["gateway_rss_mb_max"] = g["server_rss_mb_max"]
                    point = {"path": path, "concurrency": c, "direct": direct, "proxied": proxied}
                    if ngx:
                        # nginx CPU is not sampled (multi-process); compare throughput and latency.
                        nginx_res = oha(f"{ngx_url}{path}", d, c, pid=st.server.pid)
                        point["nginx"] = nginx_res
                        if direct["success_rps"]:
                            point["nginx_throughput_ratio"] = round(nginx_res["success_rps"] / direct["success_rps"], 4)
                    if direct["success_rps"]:
                        point["throughput_ratio"] = round(proxied["success_rps"] / direct["success_rps"], 4)
                    for q in ("p50_ms", "p99_ms"):
                        if direct.get(q) is not None and proxied.get(q) is not None:
                            point[f"added_{q}"] = round(proxied[q] - direct[q], 3)
                    out["points"].append(point)
        finally:
            for p in [gw] + ([ngx] if ngx else []):
                p.terminate()
                try:
                    p.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    p.kill()
    return out


def suite_tool(mode: str, smoke: bool) -> dict:
    cmd = [str(BIN / "app-bench"), mode] + (["--smoke"] if smoke else [])
    return json.loads(run(cmd, env={**os.environ, "BENCH_DATABASE_URL": BENCH_DB_URL}).stdout)


# ── main ─────────────────────────────────────────────────────────────────────────────────────


def ensure_database() -> None:
    """Create the benchmark database if missing (via app-bench: no psql/docker dependency)."""
    try:
        run([str(BIN / "app-bench"), "ensure-db"], env={**os.environ, "BENCH_PG_ADMIN_URL": PG_ADMIN_URL,
                                                       "BENCH_DATABASE_URL": BENCH_DB_URL})
    except (OSError, RuntimeError) as e:
        sys.exit(f"PostgreSQL not available ({e}); start it with ./dev up --no-app")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--suite", default=",".join(SUITES), help=f"comma list of {SUITES}")
    ap.add_argument("--smoke", action="store_true", help="short durations (CI)")
    ap.add_argument("--label", default="", help="suffix for the result file")
    ap.add_argument("--no-build", action="store_true")
    ap.add_argument("--repeat", type=int, default=None, help="runs per HTTP point, median reported (default 3; smoke 1)")
    ap.add_argument("--env", action="append", default=[], metavar="KEY=VALUE",
                    help="extra app-server environment for A/B runs (recorded in the result)")
    a = ap.parse_args()
    suites = [s.strip() for s in a.suite.split(",") if s.strip()]
    global REPEAT
    REPEAT = a.repeat if a.repeat is not None else (1 if a.smoke else 3)
    for kv in a.env:
        k, sep, v = kv.partition("=")
        if not sep or not k.startswith("APP__"):
            ap.error(f"--env expects APP__KEY=VALUE, got {kv!r}")
        EXTRA_ENV[k] = v
    unknown = set(suites) - set(SUITES)
    if unknown:
        ap.error(f"unknown suites: {sorted(unknown)}")
    if not shutil.which("oha") and {"http", "auth", "cache"} & set(suites):
        sys.exit("oha not found (brew install oha / cargo install oha)")
    if not a.no_build:
        log("building release binaries…")
        subprocess.run(["cargo", "build", "--release", "-q", "-p", "app-server", "-p", "mock-oidc", "-p", "app-bench"],
                       cwd=BACKEND, check=True, env={**os.environ, "SQLX_OFFLINE": "true"})
    ensure_database()

    started = dt.datetime.now(dt.timezone.utc)
    result: dict = {"schema": 1, "repeat": REPEAT, "server_env_overrides": EXTRA_ENV, "started_at": started.isoformat(timespec="seconds"), "smoke": a.smoke,
                    "environment": environment(), "suites": {}}
    for s in suites:
        log(f"suite {s}…")
        t = time.time()
        first_error = None
        for attempt in (1, 2):
            try:
                if s == "http":
                    data = suite_http(a.smoke)
                elif s == "auth":
                    data = suite_auth(a.smoke)
                elif s == "cache":
                    data = suite_cache(a.smoke)
                elif s == "messaging":
                    data = suite_tool("queue", a.smoke)
                elif s == "analytics":
                    data = suite_tool("analytics", a.smoke)
                elif s == "gateway":
                    data = suite_gateway(a.smoke)
                else:
                    data = suite_tool(s if s == "db" else "outbound", a.smoke)
                entry = {"status": "ok", "wall_s": round(time.time() - t, 1), "data": data}
                if first_error:
                    # One retry for transient infrastructure errors, recorded so it is visible.
                    entry["retried_after_error"] = first_error
                result["suites"][s] = entry
                break
            except Exception as e:  # noqa: BLE001 - a failed suite is recorded, never invented
                log(f"suite {s} attempt {attempt} failed: {e}")
                if attempt == 2:
                    result["suites"][s] = {"status": "failed", "error": str(e)[-2000:], "first_error": first_error}
                first_error = str(e)[-2000:]
                time.sleep(5)
    result["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds")

    RESULTS.mkdir(parents=True, exist_ok=True)
    sha = result["environment"]["git_sha"] or "nogit"
    name = f"{started:%Y%m%dT%H%M%SZ}-{sha}{'-smoke' if a.smoke else ''}{'-' + a.label if a.label else ''}.json"
    text = json.dumps(result, indent=2) + "\n"
    (RESULTS / name).write_text(text)
    # `latest.json` is the most recent *complete* run (what reports and gates read by default);
    # subsets and smoke runs get their own alias so they never replace it.
    alias = "latest-smoke.json" if a.smoke else ("latest.json" if set(suites) == set(SUITES) else "latest-partial.json")
    (RESULTS / alias).write_text(text)
    log(f"wrote benchmarks/results/{name}")
    return 0 if all(v["status"] == "ok" for v in result["suites"].values()) else 1


if __name__ == "__main__":
    sys.exit(main())
