#!/usr/bin/env python3
"""Live failure drill against a running `./dev up` stack (ideally with every module):

    python3 scripts/failure_drill.py [--api URL]          # or: ./dev test --drill

Each drill injects one real fault, checks the documented degradation, then restores it:
- provider 429 (rate limit), 5xx (errors), slow responses (timeouts), full outage (breaker);
- queue backlog (many runs at once);
- ClickHouse, NATS and PostgreSQL containers stopped;
- invalid session cookie and missing CSRF token.
Unit and integration tests cover the rest (duplicate jobs, malformed events, worker crash and
resume); see docs/operations/failure-modes.md. Modules that are not running are SKIP.
Exit status 1 if any drill fails. Containers are restarted even when a drill fails.
"""

from __future__ import annotations

import argparse
import copy
import importlib.util
import json
import re
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("system_smoke", ROOT / "scripts/system_smoke.py")
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)  # type: ignore[union-attr]
record, RESULTS = smoke.record, smoke.RESULTS
UPSTREAM = "http://127.0.0.1:59090"


def compose_project() -> str:
    m = re.search(r"COMPOSE_PROJECT_NAME:-([^}]+)\}", (ROOT / "infra/docker/compose.yaml").read_text())
    return m.group(1) if m else "app"


def container(service: str) -> str | None:
    name = f"{compose_project()}-{service}-1"
    out = subprocess.run(["docker", "ps", "--format", "{{.Names}}"], capture_output=True, text=True).stdout.split()
    return name if name in out else None


def docker(*args: str) -> None:
    subprocess.run(["docker", *args], check=True, capture_output=True)


def behaviour(**b) -> None:  # noqa: ANN003
    req = urllib.request.Request(f"{UPSTREAM}/__control", data=json.dumps(b).encode(), method="POST",
                                 headers={"content-type": "application/json"})
    urllib.request.urlopen(req, timeout=5).read()


def stats() -> dict:
    return smoke.get_json(f"{UPSTREAM}/__stats")


def run(c, slug: str, n: int, wait: float) -> dict | None:  # noqa: ANN001
    status, r = c.json("POST", f"/api/v1/orgs/{slug}/runs", {"label": "drill", "provider": "simulated", "requested": n})
    if status != 201:
        return {"status": f"HTTP {status}", "body": r}
    end = time.time() + wait
    while time.time() < end:
        _, page = c.json("GET", f"/api/v1/orgs/{slug}/runs?limit=50")
        cur = next((x for x in (page or {}).get("items", []) if x["id"] == r["id"]), None)
        if cur and cur["status"] in ("completed", "failed", "cancelled"):
            return cur
        time.sleep(0.3)
    return None


def ready(c) -> tuple[int, dict | None]:  # noqa: ANN001
    return c.json("GET", "/readyz")


def wait_ready(c, seconds: int = 60) -> bool:  # noqa: ANN001
    end = time.time() + seconds
    while time.time() < end:
        s, body = ready(c)
        if s == 200 and body and body.get("status") == "ok":
            return True
        time.sleep(1)
    return False


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--api", default="http://localhost:8080")
    a = ap.parse_args()
    c = smoke.Client(a.api)
    print(f"failure drill against {a.api}")
    if not smoke.login(c, "drill@example.com"):
        return finish()
    _, orgs = c.json("GET", "/api/v1/orgs")
    slug = next(o["slug"] for o in orgs["items"] if o["personal"])
    upstream = smoke.reachable(f"{UPSTREAM}/__stats")

    if upstream:
        try:
            # 429: the engine learns the limit and completes all work without hammering the provider.
            behaviour(rps=20.0, burst=5.0, retry_after_secs=1)
            before = stats()
            t = time.time()
            r = run(c, slug, 60, 60)
            s = stats()
            limited = s["rate_limited"] - before["rate_limited"]
            ok = r and r["status"] == "completed" and r["succeeded"] == 60
            record("PASS" if ok else "FAIL", "provider 429: run completes at the limit",
                   f"{r and r.get('succeeded')}/60 in {time.time() - t:.1f}s, {limited} 429s")
            # 5xx: retried, then counted; the run finishes, nothing hangs.
            behaviour(error_rate=0.3)
            r = run(c, slug, 40, 60)
            ok = r and r["status"] in ("completed", "failed") and r["succeeded"] + r["failed"] == 40
            record("PASS" if ok else "FAIL", "provider 5xx (30%): retried, run finishes",
                   f"status={r and r['status']} ok={r and r['succeeded']} failed={r and r['failed']}")
            # Slow provider: per-request deadline bounds the run.
            behaviour(slow_rate=1.0, slow_ms=30_000)
            t = time.time()
            r = run(c, slug, 3, 120)
            ok = r and r["status"] in ("completed", "failed") and r["failed"] == 3
            record("PASS" if ok else "FAIL", "provider timeout: deadline bounds the run",
                   f"status={r and r['status']} failed={r and r['failed']} in {time.time() - t:.0f}s")
            # Outage: the breaker opens; retries stay bounded instead of amplifying load
            # (a naive client with 3 retries sends 4 requests per call).
            behaviour(outage=True)
            before = stats()
            t = time.time()
            r = run(c, slug, 200, 90)
            sent = stats()["requests"] - before["requests"]
            ok = r and r["status"] in ("completed", "failed") and sent < 200 * 4
            record("PASS" if ok else "FAIL", "provider outage: bounded amplification",
                   f"{sent} upstream requests for 200 calls, {time.time() - t:.1f}s, status={r and r['status']}")
        finally:
            behaviour()
        # Backlog: many runs at once all drain.
        ids, t = [], time.time()
        for _ in range(25):
            s, r = c.json("POST", f"/api/v1/orgs/{slug}/runs", {"label": "backlog", "provider": "simulated", "requested": 20})
            if s == 201:
                ids.append(r["id"])
        done: set[str] = set()
        while time.time() - t < 90 and len(done) < len(ids):
            _, page = c.json("GET", f"/api/v1/orgs/{slug}/runs?limit=100")
            done |= {x["id"] for x in page["items"] if x["id"] in ids and x["status"] == "completed"}
            time.sleep(0.5)
        record("PASS" if len(done) == len(ids) == 25 else "FAIL", "queue backlog: 25 runs × 20 calls drain",
               f"{len(done)}/{len(ids)} completed in {time.time() - t:.1f}s")
    else:
        record("SKIP", "provider drills", "simulated provider not reachable")

    for service, label in (("clickhouse", "ClickHouse"), ("nats", "NATS")):
        name = container(service)
        if not name:
            record("SKIP", f"{label} outage", "module not running")
            continue
        try:
            docker("stop", name)
            time.sleep(2)  # let the client notice the disconnect
            s, body = ready(c)
            r = run(c, slug, 10, 30)
            # Optional modules are non-critical: still ready (200), but visibly degraded.
            ok = s == 200 and body and body.get("status") == "degraded" and r and r["status"] == "completed"
            record("PASS" if ok else "FAIL", f"{label} down: core keeps working",
                   f"/readyz {s} {body and body.get('status')}, run {r and r['status']}")
        finally:
            docker("start", name)
        ok = wait_ready(c, 90)
        r = run(c, slug, 5, 30)
        record("PASS" if ok and r and r["status"] == "completed" else "FAIL", f"{label} back: recovered",
               f"ready={ok} run={r and r['status']}")

    name = container("postgres")
    if name:
        try:
            docker("stop", name)
            t = time.time()
            s, _ = ready(c)
            s2, _ = c.json("GET", "/api/v1/session")
            took = time.time() - t
            ok = s == 503 and s2 >= 500 and took < 20
            record("PASS" if ok else "FAIL", "PostgreSQL down: 503, no hang, no crash",
                   f"/readyz {s}, /api/v1/session {s2}, {took:.1f}s")
        finally:
            docker("start", name)
        ok = wait_ready(c, 90)
        s, sess = c.json("GET", "/api/v1/session")
        record("PASS" if ok and s == 200 and sess.get("authenticated") else "FAIL", "PostgreSQL back: session survives",
               f"ready={ok} session={s}")
    else:
        record("SKIP", "PostgreSQL outage", "container not found")

    # Invalid session and CSRF.
    bad = smoke.Client(a.api)
    for ck in c.jar:
        if ck.name.endswith("app_session"):
            forged = copy.copy(ck)
            forged.value = "tampered" + ck.value[8:]
            bad.jar.set_cookie(forged)
    s, _ = bad.json("GET", "/api/v1/dashboard")
    record("PASS" if s == 401 else "FAIL", "tampered session cookie → 401", f"HTTP {s}")
    token, c.csrf = c.csrf, ""
    s, body = c.json("POST", f"/api/v1/orgs/{slug}/runs", {"label": "x", "provider": "simulated", "requested": 1})
    c.csrf = token
    record("PASS" if s == 403 and body and body.get("code") == "csrf_failed" else "FAIL", "missing CSRF token → 403",
           f"HTTP {s} {body and body.get('code')}")
    return finish()


def finish() -> int:
    failed = [r for r in RESULTS if r[0] == "FAIL"]
    print(f"failure drill: {'FAILED' if failed else 'OK'} — {sum(r[0] == 'PASS' for r in RESULTS)} pass, "
          f"{len(failed)} fail, {sum(r[0] == 'SKIP' for r in RESULTS)} skip")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
