#!/usr/bin/env python3
"""Create a run and wait for it, with an organization API key (Python standard library only).

    API_URL=http://localhost:8080 API_KEY=... ORG=my-org python3 runs.py

The key needs the scopes `runs:read` and `runs:create` (Organization → API keys). Exit codes:
0 success, 1 the API refused the request (the problem details are printed), 2 the run failed.
"""
from __future__ import annotations

import json
import os
import sys
import time
import urllib.error
import urllib.request

API, KEY, ORG = os.environ["API_URL"].rstrip("/"), os.environ["API_KEY"], os.environ["ORG"]


def call(method: str, path: str, body: dict | None = None) -> dict:
    req = urllib.request.Request(
        f"{API}{path}", method=method, data=json.dumps(body).encode() if body is not None else None,
        headers={"authorization": f"Bearer {KEY}", "accept": "application/json", "content-type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=15) as res:
            return json.load(res)
    except urllib.error.HTTPError as e:
        # Errors are RFC 9457 problem details; quote x-request-id when reporting a problem.
        problem = json.loads(e.read() or b"{}")
        print(f"{method} {path}: {e.code} {problem.get('code')}: {problem.get('detail') or problem.get('title')}"
              f" {problem.get('errors') or ''} (request {e.headers.get('x-request-id')})", file=sys.stderr)
        sys.exit(1)


recent = call("GET", f"/api/v1/orgs/{ORG}/runs?limit=5")["items"]
print(f"{len(recent)} recent run(s)")
run = call("POST", f"/api/v1/orgs/{ORG}/runs", {"label": "from runs.py", "provider": "simulated", "requested": 3})
print(f"created run {run['id']} ({run['status']})")
deadline = time.time() + 60
while run["status"] in ("queued", "running") and time.time() < deadline:
    time.sleep(1)
    run = call("GET", f"/api/v1/orgs/{ORG}/runs/{run['id']}")
print(f"run {run['id']}: {run['status']}, {run['succeeded']}/{run['requested']} calls succeeded")
sys.exit(0 if run["status"] == "completed" else 2)
