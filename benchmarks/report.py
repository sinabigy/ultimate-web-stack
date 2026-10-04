#!/usr/bin/env python3
"""Render a benchmark result as Markdown (docs/benchmarks/).

    python3 benchmarks/report.py                       # results/latest.json → docs/benchmarks/latest.md
    python3 benchmarks/report.py results/X.json -o -   # to stdout

Everything in the report comes from the result file; nothing is computed that was not measured
except simple ratios, which are labelled as such.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent


def f(v, digits=1, suffix=""):
    if v is None:
        return "–"
    if isinstance(v, float):
        if abs(v) >= 1000:
            return f"{v:,.0f}{suffix}"
        return f"{v:.{digits}f}{suffix}"
    if isinstance(v, int):
        return f"{v:,}{suffix}"
    return f"{v}{suffix}"


def pct(v, digits=1):
    return "–" if v is None else f"{v * 100:.{digits}f}%"


def table(headers: list[str], rows: list[list]) -> str:
    out = ["| " + " | ".join(headers) + " |", "|" + "|".join("---:" if i else "---" for i in range(len(headers))) + "|"]
    out += ["| " + " | ".join(str(c) for c in r) + " |" for r in rows]
    return "\n".join(out)


def http_row(name: str, r: dict) -> list:
    return [name, r.get("concurrency"), f(r.get("success_rps"), 0), pct(r.get("success_rate"), 2), f(r.get("p50_ms"), 2),
            f(r.get("p99_ms"), 2), f(r.get("p999_ms"), 2), f(r.get("server_cpu_pct_avg"), 0, "%"), f(r.get("server_rss_mb_max"), 1),
            f(r.get("success_rps_per_server_core"), 0)]


HTTP_HEADERS = ["endpoint", "conc.", "success req/s", "success", "p50 ms", "p99 ms", "p99.9 ms", "server CPU", "RSS MB", "req/s per core"]


def render(res: dict, source: str) -> str:
    e = res["environment"]
    s = res["suites"]
    md = [f"# Benchmark results{' (smoke)' if res.get('smoke') else ''}", "",
          f"Generated from `benchmarks/results/{source}` by `benchmarks/report.py`. Do not edit by hand.", "",
          "## Environment", "",
          table(["", ""], [
              ["when", f"{res['started_at']} → {res.get('finished_at', '?')}"],
              ["machine", f"{e.get('cpu')} · {e.get('logical_cores')} logical cores · {e.get('memory_gb')} GB · {e.get('os')} · {e.get('arch')}"],
              ["services", f"container VM: {e.get('container_runtime')}"],
              ["build", f"{e.get('build_profile')} · {e.get('rustc')}"],
              ["code", f"`{e.get('git_sha')}`{' (uncommitted changes)' if e.get('git_dirty') else ''}"],
              ["load generator", e.get("oha")],
          ]), "", f"> {e.get('notes')}", ""]

    for name, v in s.items():
        if v["status"] != "ok":
            md += [f"## {name}", "", f"**Not measured** — suite {v['status']}: `{v.get('error', '')[:300]}`", ""]

    if s.get("http", {}).get("status") == "ok":
        d = s["http"]["data"]
        rows = [http_row(f"`/bench/{n}`", r) for n in ("plaintext", "json") for r in d[n]]
        rows += [http_row("`/bench/db` (1 indexed read)", d["db"]), http_row("`/bench/updates` (1 read-modify-write)", d["updates"]),
                 http_row("`/bench/cached` (memory cache)", d["cached"]), http_row("`/healthz`", d["healthz"])]
        md += ["## HTTP server", "", f"oha, {d['duration_per_point']} per point, keep-alive, full middleware stack "
               "(request id, tracing, security headers, CSRF, metrics, timeouts, load shedding).", "", table(HTTP_HEADERS, rows), "",
               "`req/s per core` = success req/s ÷ (server CPU% / 100): a derived ratio, sensitive to `ps` sampling.", ""]

    if s.get("auth", {}).get("status") == "ok":
        d = s["auth"]["data"]
        rows = []
        for key, label in [("profile", "`GET /api/v1/account/profile`"), ("session", "`GET /api/v1/session`"),
                           ("org_runs_page", "`GET /api/v1/orgs/{slug}/runs` (tenant page of 20)"),
                           ("unauthenticated_401", "no credentials → 401"), ("cross_tenant_404", "non-member org → 404 (audited)")]:
            r = d[key]
            row = http_row(label, r)
            row[3] = ", ".join(f"{k}: {v:,}" for k, v in r["status_codes"].items())
            rows.append(row[:1] + [r.get("db_queries", "–")] + row[1:])
        hdr = HTTP_HEADERS[:1] + ["DB round trips"] + HTTP_HEADERS[1:]
        hdr[4] = "status codes"
        md += ["## Authenticated requests", "", "Real cookie → session → authorization path (BFF session cookie, no mocks).", "",
               table(hdr, rows), ""]

    if s.get("cache", {}).get("status") == "ok":
        d = s["cache"]["data"]
        rows = []
        for k in ("memory", "redis", "dragonfly"):
            r = d.get(k, {})
            rows.append([k, r["skipped"]] + [""] * 9 if "skipped" in r else http_row(k, r))
        md += ["## Cache backends", "", f"{d['endpoint']}, {d['duration_per_point']} per backend.", "",
               table(["backend"] + HTTP_HEADERS[1:], rows), ""]

    if s.get("outbound", {}).get("status") == "ok":
        d = s["outbound"]["data"]
        cr = d["connection_reuse"]
        md += ["## Outbound engine", "", "Simulated provider on loopback (`fake-upstream`). *Naive* = fixed concurrency, "
               "immediate retry on any failure (up to 20), no rate/concurrency control, no breaker.", "",
               "### Connection reuse", "",
               table(["client", "requests", "TCP connections", "reuse", "useful req/s", "p50 ms", "p99 ms"], [
                   [k, f(v["requests"]), f(v["upstream"]["connections"]), pct(v["upstream"]["connection_reuse"]), f(v["useful_rps"], 0),
                    f(v["p50_ms"], 2), f(v["p99_ms"], 2)] for k, v in cr.items()]), "",
               "### Concurrency sweep (provider latency 5 ms)", "",
               table(["concurrency", "requests", "useful req/s", "p50 ms", "p99 ms", "p99.9 ms"], [
                   [r["concurrency"], f(r["requests"]), f(r["useful_rps"], 0), f(r["p50_ms"], 2), f(r["p99_ms"], 2), f(r["p999_ms"], 2)]
                   for r in d["concurrency_sweep_5ms"]]), ""]
        for key, title in [("rate_limited_500rps", "Rate-limited provider (500 req/s, burst 50, 429 + Retry-After: 1)"),
                           ("overloaded_capacity16", "Overloaded provider (16 concurrent at 10 ms ≈ 1,600 req/s capacity; 503 above 48 in flight)")]:
            rows = []
            for client, v in d[key].items():
                u = v["upstream"]
                rows.append([client, f(v["requests"]), pct(v["success_rate"]), f(v["wall_s"], 2), f(v["useful_rps"], 0), f(u["sent"]),
                             f(u["rejected_429"] + u["rejected_503"]), pct(u["waste_ratio"]), f(v["p99_ms"], 1)])
            md += [f"### {title}", "", table(["client", "work items", "completed", "wall s", "useful req/s", "requests sent",
                                               "rejected", "waste", "p99 ms"], rows), ""]
        rows = []
        for client in ("engine", "naive"):
            v = d.get(f"outage_open_loop_{client}")
            if v:
                rows.append([client, f(v["offered"]), f(v["succeeded"]), f(v["upstream_rejected_503"]),
                             f(v["amplification_during_outage"], 2, "×"), f(v["recovery_s"], 3, " s")])
        if rows:
            v = d["outage_open_loop_engine"]
            md += [f"### Provider outage (open loop {v['offered_rps']} req/s for {v['duration_s']} s; 503 from "
                   f"{v['outage_window_s'][0]:g}–{v['outage_window_s'][1]:g} s)", "",
                   table(["client", "offered", "succeeded", "requests to dead provider", "amplification", "recovery"], rows), "",
                   "Amplification = requests sent to the provider during the outage ÷ requests offered in that window.", ""]

    if s.get("db", {}).get("status") == "ok":
        d = s["db"]["data"]
        rows = [[f"`{r['query']}`", r["pool_size"], r["concurrency"], f(r["ops_per_sec"], 0), f(r["p50_ms"], 2), f(r["p99_ms"], 2),
                 f(r["pool_wait_p50_ms"], 2), f(r["pool_wait_p99_ms"], 2)] for r in d["results"]]
        md += ["## PostgreSQL", "", f"`app-bench db`, {d['duration_s']:g} s per point. Latency includes pool acquire.", "",
               table(["workload", "pool", "workers", "ops/s", "p50 ms", "p99 ms", "pool wait p50", "pool wait p99"], rows), ""]

    gate = subprocess.run([sys.executable, str(HERE / "compare.py"), str(HERE / "results" / source)] + (["--smoke"] if res.get("smoke") else []),
                          text=True, capture_output=True)
    md += ["## Gates", "", "```", gate.stdout.strip(), "```", ""]
    return "\n".join(md)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("result", nargs="?", default=str(HERE / "results" / "latest.json"))
    ap.add_argument("-o", "--output", default=str(ROOT / "docs" / "benchmarks" / "latest.md"))
    a = ap.parse_args()
    src = Path(a.result)
    res = json.loads(src.read_text())
    # Name the immutable result file, not the `latest` alias, so the report stays traceable.
    source = src.name
    if source.startswith("latest"):
        twin = next((p for p in sorted((HERE / "results").glob("2*.json"), reverse=True) if p.read_text() == src.read_text()), None)
        source = twin.name if twin else source
    text = render(res, source)
    if a.output == "-":
        print(text)
    else:
        Path(a.output).parent.mkdir(parents=True, exist_ok=True)
        Path(a.output).write_text(text + "\n")
        print(f"wrote {a.output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
