#!/usr/bin/env python3
"""Release-profile experiment: LTO, codegen units, allocators and PGO for app-server.

    python3 benchmarks/release_profile.py [--variants baseline,nolto,...] [--smoke]

Each variant is built into its own target directory (build time and binary size recorded),
then measured with `benchmarks/run.py --suite http --server-bin <binary>` (median of 3).
PGO: an instrumented build is trained under the same HTTP load, profiles are merged with
llvm-profdata, and the optimised build uses them. Writes
benchmarks/results/<UTC>-<sha>-release-profile.json (one entry per variant, linking to the
per-variant run files).
"""

from __future__ import annotations

import argparse
import datetime as dt
import glob
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BACKEND = ROOT / "backend"
RESULTS = ROOT / "benchmarks" / "results"
VARIANT_DIR = ROOT / "var" / "release-variants"

# name → (cargo profile, features, extra RUSTFLAGS, description)
VARIANTS = {
    "baseline": ("release", "", "", "thin LTO, codegen-units=1 (shipped)"),
    "nolto": ("release-nolto", "", "", "no LTO, codegen-units=16"),
    "fatlto": ("release-fatlto", "", "", "fat LTO, codegen-units=1"),
    "mimalloc": ("release", "alloc-mimalloc", "", "baseline + mimalloc"),
    "jemalloc": ("release", "alloc-jemalloc", "", "baseline + jemalloc"),
    "pgo": ("release", "", "PGO", "baseline + profile-guided optimisation"),
}


def log(msg: str) -> None:
    print(f"[release] {msg}", file=sys.stderr, flush=True)


def host_triple() -> str:
    out = subprocess.run(["rustc", "-vV"], capture_output=True, text=True, check=True).stdout
    return next(line.split(": ", 1)[1] for line in out.splitlines() if line.startswith("host: "))


def build(name: str, profile: str, features: str, rustflags: str) -> tuple[Path, float]:
    """Build one variant (timed) and cache its build facts in var/release-variants/<name>.build.json."""
    target = VARIANT_DIR / name
    env = {**os.environ, "SQLX_OFFLINE": "true", "CARGO_TARGET_DIR": str(target)}
    cmd = ["cargo", "build", "-q", "--profile", profile, "-p", "app-server"] + (["--features", features] if features else [])
    out_dir = target / profile
    if rustflags:
        # An explicit --target keeps RUSTFLAGS off build scripts and proc macros (instrumented
        # proc-macro dylibs crash rustc: observed SIGSEGV compiling sqlx with -Cprofile-generate).
        env["RUSTFLAGS"] = rustflags
        triple = host_triple()
        cmd += ["--target", triple]
        out_dir = target / triple / profile
    t = time.time()
    subprocess.run(cmd, cwd=BACKEND, env=env, check=True)
    elapsed = time.time() - t
    binary = out_dir / "app-server"
    (VARIANT_DIR / f"{name}.build.json").write_text(json.dumps({"build_s": round(elapsed, 1), "binary": str(binary)}))
    return binary, elapsed


def cached_build(name: str) -> tuple[Path, float] | None:
    f = VARIANT_DIR / f"{name}.build.json"
    if not f.exists():
        return None
    d = json.loads(f.read_text())
    b = Path(d["binary"])
    return (b, d["build_s"]) if b.exists() else None


def measure(name: str, binary: Path, smoke: bool) -> Path:
    label = f"release-{name}"
    cmd = [sys.executable, str(ROOT / "benchmarks" / "run.py"), "--no-build", "--suite", "http", "--label", label,
           "--server-bin", str(binary)] + (["--smoke"] if smoke else [])
    subprocess.run(cmd, check=True)
    files = sorted(RESULTS.glob(f"*-{label}.json"))
    return files[-1]


def profdata_tool() -> str:
    found = glob.glob(str(Path.home() / ".rustup/toolchains/1.99.0-*/lib/rustlib/*/bin/llvm-profdata"))
    if not found:
        sys.exit("llvm-profdata not found: rustup component add llvm-tools --toolchain 1.99.0")
    return found[0]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--variants", default=",".join(VARIANTS))
    ap.add_argument("--smoke", action="store_true")
    ap.add_argument("--reuse-builds", action="store_true", help="use cached variant builds (var/release-variants/*.build.json)")
    ap.add_argument("--reuse-results", action="store_true", help="use the latest existing results/*-release-<variant>.json")
    a = ap.parse_args()
    names = [v.strip() for v in a.variants.split(",") if v.strip()]
    entries = []
    for name in names:
        profile, features, rustflags, desc = VARIANTS[name]
        log(f"variant {name}: {desc}")
        cached = cached_build(name) if a.reuse_builds else None
        if cached:
            binary, build_s = cached
            log(f"  reusing build {binary} (cold build {build_s:.0f}s)")
        elif rustflags == "PGO":
            data = VARIANT_DIR / "pgo-data"
            shutil.rmtree(data, ignore_errors=True)
            data.mkdir(parents=True)
            log("  PGO 1/3: instrumented build")
            # Instrument without LTO: with LTO the instrumentation was lowered by a different LLVM
            # than the profiler runtime ("Runtime and instrumentation version mismatch: expected 11,
            # but get 10") and no profile was written. The optimised build below keeps thin LTO.
            instrumented, t_gen = build("pgo-gen", "release-nolto", features, f"-Cprofile-generate={data}")
            log("  PGO 2/3: training under the HTTP benchmark load")
            subprocess.run([sys.executable, str(ROOT / "benchmarks" / "run.py"), "--no-build", "--suite", "http,auth", "--smoke",
                            "--label", "pgo-training", "--server-bin", str(instrumented)], check=True)
            for f in RESULTS.glob("*-pgo-training*.json"):
                f.unlink()
            merged = VARIANT_DIR / "pgo.profdata"
            subprocess.run([profdata_tool(), "merge", "-o", str(merged), *map(str, data.glob("*.profraw"))], check=True)
            log("  PGO 3/3: optimised build")
            binary, t_use = build("pgo-use", profile, features, f"-Cprofile-use={merged}")
            build_s = t_gen + t_use
        else:
            binary, build_s = build(name, profile, features, rustflags)
        size = binary.stat().st_size
        existing = sorted(RESULTS.glob(f"*-release-{name}.json")) if a.reuse_results else []
        if existing:
            result = existing[-1]
            log(f"  reusing measurement {result.name}")
        else:
            log(f"  built in {build_s:.0f}s, {size / 1e6:.1f} MB; measuring")
            result = measure(name, binary, a.smoke)
        data = json.loads(result.read_text())["suites"]["http"]
        entries.append({"variant": name, "description": desc, "profile": profile, "features": features,
                        "build_s": round(build_s, 1), "binary_bytes": size, "result_file": result.name,
                        "status": data["status"], "http": data.get("data")})
    sha = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "--short=12", "HEAD"], text=True, capture_output=True).stdout.strip()
    started = dt.datetime.now(dt.timezone.utc)
    out = RESULTS / f"{started:%Y%m%dT%H%M%SZ}-{sha}-release-profile{'-smoke' if a.smoke else ''}.json"
    out.write_text(json.dumps({"schema": 1, "kind": "release-profile", "smoke": a.smoke, "variants": entries}, indent=2) + "\n")
    log(f"wrote {out.relative_to(ROOT)}")
    print(f"{'variant':10} {'build s':>8} {'size MB':>8} {'plain64':>9} {'json64':>9} {'json256':>9} {'cached':>9} {'db':>8} {'RSS MB':>7}")
    for e in entries:
        h = e["http"] or {}
        pick = lambda k, c=None: next((x["success_rps"] for x in h.get(k, []) if x["concurrency"] == c), None) if c else (h.get(k) or {}).get("success_rps")  # noqa: E731
        rss = next((x.get("server_rss_mb_max") for x in h.get("plaintext", []) if x["concurrency"] == 64), None)
        print(f"{e['variant']:10} {e['build_s']:>8} {e['binary_bytes'] / 1e6:>8.1f} {pick('plaintext', 64) or 0:>9.0f} "
              f"{pick('json', 64) or 0:>9.0f} {pick('json', 256) or 0:>9.0f} {pick('cached') or 0:>9.0f} {pick('db') or 0:>8.0f} {rss or 0:>7}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
