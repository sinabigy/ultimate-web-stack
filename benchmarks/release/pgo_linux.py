#!/usr/bin/env python3
"""PGO on Linux (release-profile experiment): baseline vs profile-guided build of app-server.

    python3 benchmarks/release/pgo_linux.py

Runs in rust:1.99 (Debian) with the database reached through the host gateway (app_bench on
the dev PostgreSQL). Baseline and PGO are measured alternately (A B A B) with oha at c=64 on
the bench endpoints. Writes benchmarks/results/<UTC>-<sha>-pgo-linux.json.
"""
import datetime as dt
import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "var" / "pgo-linux"


def main() -> int:
    shutil.rmtree(OUT, ignore_errors=True)
    (OUT / "raw").mkdir(parents=True)
    cmd = ["docker", "run", "--rm", "--add-host=host.docker.internal:host-gateway",
           "-e", "DB_URL=postgres://app:app-dev-only@host.docker.internal:55432/app_bench",
           "-v", f"{ROOT / 'backend'}:/src", "-v", "uwsb-pgo-cargo:/usr/local/cargo/registry", "-v", "uwsb-pgo-target:/t",
           "-v", f"{Path(__file__).parent / 'pgo_inside.sh'}:/pgo_inside.sh:ro", "-v", f"{OUT}:/out",
           "rust:1.99.0-bookworm", "bash", "/pgo_inside.sh"]
    if subprocess.run(cmd).returncode != 0:
        return 1
    env = dict(l.split("=", 1) for l in (OUT / "env.txt").read_text().splitlines() if "=" in l)
    rows = []
    for f in sorted((OUT / "raw").glob("*.json")):
        name, path, rnd = f.stem.rsplit("-", 2)
        raw = json.loads(f.read_text())
        codes = raw.get("statusCodeDistribution") or {}
        ok = sum(v for k, v in codes.items() if str(k).startswith("2"))
        lat = raw.get("latencyPercentiles", {})
        rows.append({"variant": name, "endpoint": path, "round": int(rnd[1:]), "success_rps": round(ok / raw["summary"]["total"], 1),
                     "p50_ms": round(lat.get("p50", 0) * 1000, 3), "p99_ms": round(lat.get("p99", 0) * 1000, 3)})
    sha = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "--short=12", "HEAD"], capture_output=True, text=True).stdout.strip()
    now = dt.datetime.now(dt.timezone.utc)
    dest = ROOT / "benchmarks/results" / f"{now:%Y%m%dT%H%M%SZ}-{sha}-pgo-linux.json"
    dest.write_text(json.dumps({"schema": 1, "kind": "pgo-linux", "environment": {**env, "image": "rust:1.99.0-bookworm",
                                "note": "server, load generator in one 2-vCPU container; PostgreSQL via host gateway"},
                                "results": rows}, indent=2) + "\n")
    print(f"wrote {dest.relative_to(ROOT)}")
    for ep in ["plaintext", "json", "db", "cached"]:
        b = [r["success_rps"] for r in rows if r["variant"] == "baseline" and r["endpoint"] == ep]
        p = [r["success_rps"] for r in rows if r["variant"] == "pgo" and r["endpoint"] == ep]
        print(f"{ep:10} baseline {b}  pgo {p}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
