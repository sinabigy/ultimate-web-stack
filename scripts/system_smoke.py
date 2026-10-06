#!/usr/bin/env python3
"""System smoke test against a running stack (`./dev up`, ideally with every module):

    ./dev test --system            # or: python3 scripts/system_smoke.py [--api URL] [--json]

One user journey through every component:
- the BFF login against the mock IdP (cookie session, CSRF);
- PostgreSQL;
- a run created with a known W3C trace id, which goes through job queue → worker → outbound
  engine → simulated provider;
- realtime SSE event;
- NATS (when messaging is enabled);
- ClickHouse analytics (when analytics is enabled);
- cache read;
- audit trail;
- metrics;
- a Tempo trace (when observability runs).

Modules that are not running are reported SKIP, never PASS. Exit status 1 if any check fails.
"""

from __future__ import annotations

import argparse
import http.cookiejar
import json
import os
import secrets
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

RESULTS: list[tuple[str, str, str]] = []


def record(status: str, name: str, detail: str = "") -> None:
    RESULTS.append((status, name, detail))
    print(f"  {status:4}  {name:44} {detail}", flush=True)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):  # noqa: ANN002, ANN003
        return None


class Client:
    def __init__(self, api: str):
        self.api = api.rstrip("/")
        self.jar = http.cookiejar.CookieJar()
        self.opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self.jar), NoRedirect())
        self.csrf = ""

    def req(self, method: str, url: str, body=None, headers=None, form=False):  # noqa: ANN001
        if not url.startswith("http"):
            url = self.api + url
        data = None
        h = dict(headers or {})
        if body is not None:
            if form:
                data = urllib.parse.urlencode(body).encode()
                h["content-type"] = "application/x-www-form-urlencoded"
            else:
                data = json.dumps(body).encode()
                h["content-type"] = "application/json"
        if method not in ("GET", "HEAD") and self.csrf:
            h["x-csrf-token"] = self.csrf
        r = urllib.request.Request(url, data=data, method=method, headers=h)
        try:
            with self.opener.open(r, timeout=15) as resp:
                raw = resp.read()
                return resp.status, dict(resp.headers), raw
        except urllib.error.HTTPError as e:
            return e.code, dict(e.headers), e.read()
        except urllib.error.URLError as e:
            # Not retried: name the request so a transient failure (e.g. name resolution) is diagnosable.
            raise ConnectionError(f"{method} {url}: {e.reason}") from e

    def json(self, method: str, url: str, body=None, headers=None):  # noqa: ANN001
        status, h, raw = self.req(method, url, body, headers)
        try:
            return status, json.loads(raw or b"null")
        except json.JSONDecodeError:
            return status, None


def reachable(url: str) -> bool:
    try:
        with urllib.request.urlopen(url, timeout=2):
            return True
    except Exception:  # noqa: BLE001
        return False


def get_json(url: str):  # noqa: ANN201
    try:
        with urllib.request.urlopen(url, timeout=5) as r:
            return json.loads(r.read())
    except urllib.error.URLError as e:
        raise ConnectionError(f"GET {url}: {getattr(e, 'reason', e)}") from e


def login(c: Client, email: str) -> bool:
    status, h, _ = c.req("GET", "/auth/login?return_to=/dashboard")
    loc = h.get("Location") or h.get("location")
    if status not in (302, 303) or not loc:
        record("FAIL", "login: start (BFF → IdP redirect)", f"HTTP {status}")
        return False
    authorize = urllib.parse.urlparse(loc)
    params = {k: v for k, v in urllib.parse.parse_qsl(authorize.query) if k not in ("prompt", "login_hint", "max_age")}
    params.update({"email": email, "amr": "mfa", "roles": ""})
    issuer = f"{authorize.scheme}://{authorize.netloc}"
    status, h, body = c.req("POST", f"{issuer}/authorize/complete", params, form=True)
    cb = h.get("Location") or h.get("location")
    if status not in (302, 303) or not cb:
        record("FAIL", "login: identity provider", f"HTTP {status} {body[:120]!r}")
        return False
    cbu = urllib.parse.urlparse(cb)
    status, h, _ = c.req("GET", f"{cbu.path}?{cbu.query}")
    if status not in (302, 303) or not any(ck.name.endswith("app_session") for ck in c.jar):
        record("FAIL", "login: callback sets session cookie", f"HTTP {status}")
        return False
    status, s = c.json("GET", "/api/v1/session")
    if status != 200 or not s or not s.get("authenticated"):
        record("FAIL", "login: session", f"HTTP {status}")
        return False
    c.csrf = s["csrf_token"]
    record("PASS", "login (BFF + OIDC + PKCE, cookie session)", email)
    return True


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--api", default=os.environ.get("SMOKE_API", "http://localhost:8080"))
    ap.add_argument("--upstream", default="http://127.0.0.1:59090")
    ap.add_argument("--nats-monitor", default="http://127.0.0.1:58222")
    ap.add_argument("--tempo", default="http://127.0.0.1:53200")
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()
    c = Client(a.api)
    print(f"system smoke against {a.api}")

    status, ready = c.json("GET", "/readyz")
    if status != 200:
        record("FAIL", "readiness", f"HTTP {status} {ready}")
        return finish(a)
    checks = {x["name"]: x["ok"] for x in (ready or {}).get("checks", [])}
    record("PASS", "readiness (critical dependencies up)", ", ".join(f"{k}={'ok' if v else 'DEGRADED'}" for k, v in checks.items()))

    email = f"smoke-{secrets.token_hex(4)}@example.com"
    if not login(c, email):
        return finish(a)
    status, orgs = c.json("GET", "/api/v1/orgs")
    slug = next(o["slug"] for o in orgs["items"] if o["personal"])

    # Realtime: subscribe before creating the run.
    events: list[dict] = []
    def sse() -> None:
        cookie = "; ".join(f"{ck.name}={ck.value}" for ck in c.jar)
        r = urllib.request.Request(a.api + "/api/v1/events", headers={"cookie": cookie, "accept": "text/event-stream"})
        try:
            with urllib.request.urlopen(r, timeout=30) as resp:
                for line in resp:
                    if line.startswith(b"data:"):
                        try:
                            events.append(json.loads(line[5:].strip()))
                        except json.JSONDecodeError:
                            pass
                    if any(e.get("type") == "run_finished" for e in events):
                        return
        except Exception:  # noqa: BLE001
            return
    t = threading.Thread(target=sse, daemon=True)
    t.start()
    time.sleep(0.5)

    nats_before = None
    if reachable(f"{a.nats_monitor}/healthz"):
        nats_before = get_json(f"{a.nats_monitor}/varz")["in_msgs"]
    upstream_before = get_json(f"{a.upstream}/__stats")["requests"] if reachable(f"{a.upstream}/__stats") else None

    trace_id = secrets.token_hex(16)
    status, run = c.json("POST", f"/api/v1/orgs/{slug}/runs", {"label": "system smoke", "provider": "simulated", "requested": 20},
                         {"traceparent": f"00-{trace_id}-{secrets.token_hex(8)}-01"})
    if status != 201:
        record("FAIL", "create run (PostgreSQL + job enqueue)", f"HTTP {status} {run}")
        return finish(a)
    record("PASS", "create run (PostgreSQL + job enqueue, audited)", run["id"])

    deadline = time.time() + 30
    final = None
    while time.time() < deadline:
        status, page = c.json("GET", f"/api/v1/orgs/{slug}/runs")
        final = next((r for r in page["items"] if r["id"] == run["id"]), None)
        if final and final["status"] in ("completed", "failed"):
            break
        time.sleep(0.3)
    if final and final["status"] == "completed" and final["succeeded"] == 20:
        record("PASS", "worker → outbound engine → provider", f"{final['succeeded']}/20 calls succeeded")
    else:
        record("FAIL", "worker → outbound engine → provider", f"run={final}")

    if upstream_before is not None:
        sent = get_json(f"{a.upstream}/__stats")["requests"] - upstream_before
        record("PASS" if sent >= 20 else "FAIL", "simulated provider received the calls", f"{sent} requests")
        ids = get_json(f"{a.upstream}/__stats").get("trace_ids", [])
        record("PASS" if trace_id in ids else "FAIL", "trace context reached the provider", trace_id)
    else:
        record("SKIP", "simulated provider", "fake-upstream not reachable")

    t.join(timeout=10)
    got = [e for e in events if e.get("type") == "run_finished" and e.get("run_id") == run["id"]]
    record("PASS" if got else "FAIL", "realtime event over SSE", f"{len(events)} event(s) received")

    if nats_before is not None:
        delta = get_json(f"{a.nats_monitor}/varz")["in_msgs"] - nats_before
        record("PASS" if delta > 0 else "FAIL", "NATS carried realtime events", f"{delta} messages")
    else:
        record("SKIP", "NATS", "monitor not reachable (messaging module off)")

    status, an = c.json("GET", f"/api/v1/orgs/{slug}/analytics/runs?days=1")
    if status == 404:
        record("SKIP", "analytics (ClickHouse)", "module off")
    else:
        found = False
        for _ in range(30):
            status, an = c.json("GET", f"/api/v1/orgs/{slug}/analytics/runs?days=1")
            if status == 200 and an["created"] and an["finished"]:
                found = True
                break
            time.sleep(0.5)
        record("PASS" if found else "FAIL", "analytics events in ClickHouse", json.dumps(an)[:100] if an else f"HTTP {status}")

    s1, _ = c.json("GET", "/bench/cached")
    if s1 == 200:
        s2, _ = c.json("GET", "/bench/cached")
        record("PASS" if s2 == 200 else "FAIL", "cache read path", "")
    else:
        record("SKIP", "cache read path", "bench endpoints disabled")

    status, audit = c.json("GET", f"/api/v1/orgs/{slug}/audit")
    actions = {e["action"] for e in (audit or {}).get("items", [])} if status == 200 else set()
    record("PASS" if "run.created" in actions else "FAIL", "audit trail records the run", ", ".join(sorted(actions))[:80])

    status, h, raw = c.req("GET", "/metrics")
    if status == 200:
        text = raw.decode(errors="replace")
        ok = "app_provider_requests_total" in text and "app_http_requests_total" in text
        record("PASS" if ok else "FAIL", "metrics (HTTP + provider series)", "")
    else:
        record("SKIP", "metrics", "not on the public port (ops port configured)")

    if reachable(f"{a.tempo}/ready"):
        spans = 0
        for _ in range(20):
            try:
                tr = get_json(f"{a.tempo}/api/traces/{trace_id}")
                spans = sum(len(ss["spans"]) for b in tr.get("batches", []) for ss in b.get("scopeSpans", []))
            except Exception:  # noqa: BLE001
                spans = 0
            if spans >= 3:
                break
            time.sleep(1)
        record("PASS" if spans >= 3 else "FAIL", "trace in Tempo (request → job → provider)", f"{spans} spans")
    else:
        record("SKIP", "Tempo trace", "observability module not running")
    return finish(a)


def finish(a: argparse.Namespace) -> int:
    failed = [r for r in RESULTS if r[0] == "FAIL"]
    if a.json:
        print(json.dumps([{"status": s, "check": n, "detail": d} for s, n, d in RESULTS], indent=2))
    print(f"system smoke: {'FAILED' if failed else 'OK'} — {sum(r[0] == 'PASS' for r in RESULTS)} pass, "
          f"{len(failed)} fail, {sum(r[0] == 'SKIP' for r in RESULTS)} skip")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
