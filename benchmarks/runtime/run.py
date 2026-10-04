#!/usr/bin/env python3
"""Runtime experiment (T-0013): Tokio vs Monoio, epoll vs io_uring, in a Linux container.

    python3 benchmarks/runtime/run.py [--smoke]

Runs backend/experiments/runtime in a Debian container with seccomp unconfined (Docker's default
seccomp profile blocks io_uring), load generator inside the same container. Writes
benchmarks/results/<UTC>-<sha>-runtime.json. Every variant serves the same minimal HTTP/1.1
response with the same parsing code, so differences are the runtime/driver, not the app.
"""

import argparse
import datetime as dt
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
IMAGE = "rust:1.99.0-bookworm"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--smoke", action="store_true")
    a = ap.parse_args()
    # Under the repository (gitignored var/): colima/Docker Desktop share $HOME, not /var/folders.
    out = ROOT / "var" / "runtime-bench"
    if out.exists():
        for f in out.rglob("*.json"):
            f.unlink()
    out.mkdir(parents=True, exist_ok=True)
    env = ["-e", "DURATION=3s", "-e", "CONCS=64", "-e", "THREADS_LIST=1"] if a.smoke else []
    cmd = ["docker", "run", "--rm", "--security-opt", "seccomp=unconfined",
           "-v", f"{ROOT / 'backend/experiments/runtime'}:/src",
           "-v", f"{Path(__file__).parent / 'inside.sh'}:/inside.sh:ro",
           "-v", "uwsb-runtime-cargo:/usr/local/cargo/registry",
           "-v", "uwsb-runtime-target:/src/target",
           "-v", f"{out}:/out", *env, IMAGE, "bash", "/inside.sh"]
    print("$", " ".join(cmd), file=sys.stderr)
    r = subprocess.run(cmd, text=True)
    if r.returncode != 0:
        print("container run failed", file=sys.stderr)
        return 1
    envd = dict(line.split("=", 1) for line in (out / "env.txt").read_text().splitlines() if "=" in line)
    rows = []
    for f in sorted((out / "raw").glob("*.json")):
        name, t, c = f.stem.rsplit("-", 2)
        raw = json.loads(f.read_text() or "{}")
        if not raw:
            rows.append({"variant": name, "threads": int(t[1:]), "concurrency": int(c[1:]), "error": "no result"})
            continue
        s, lat = raw["summary"], raw.get("latencyPercentiles", {})
        codes = raw.get("statusCodeDistribution") or {}
        ok = sum(v for k, v in codes.items() if str(k).startswith("2"))
        rows.append({
            "variant": name, "threads": int(t[1:]), "concurrency": int(c[1:]),
            "success_rps": round(ok / s["total"], 1), "success_rate": s.get("successRate"),
            "p50_ms": round(lat.get("p50", 0) * 1000, 3), "p99_ms": round(lat.get("p99", 0) * 1000, 3),
            "p999_ms": round(lat.get("p99.9", 0) * 1000, 3),
        })
    sha = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "--short=12", "HEAD"], text=True, capture_output=True).stdout.strip()
    started = dt.datetime.now(dt.timezone.utc)
    result = {"schema": 1, "kind": "runtime-experiment", "smoke": a.smoke, "started_at": started.isoformat(timespec="seconds"),
              "environment": {**envd, "image": IMAGE, "host": "colima VM on Apple M5", "seccomp": "unconfined",
                              "note": "load generator in the same container; 2 vCPUs shared by server and oha"},
              "results": rows}
    dest = ROOT / "benchmarks" / "results" / f"{started:%Y%m%dT%H%M%SZ}-{sha}-runtime{'-smoke' if a.smoke else ''}.json"
    dest.write_text(json.dumps(result, indent=2) + "\n")
    print(f"wrote {dest.relative_to(ROOT)}", file=sys.stderr)
    print(f"{'variant':14} thr conc {'req/s':>10} {'p50 ms':>8} {'p99 ms':>8}")
    for r in rows:
        print(f"{r['variant']:14} {r['threads']:>3} {r['concurrency']:>4} {r.get('success_rps', 0):>10} {r.get('p50_ms', 0):>8} {r.get('p99_ms', 0):>8}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
