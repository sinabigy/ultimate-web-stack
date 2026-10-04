#!/usr/bin/env python3
"""Benchmark regression gates.

    python3 benchmarks/compare.py                         # results/latest.json vs baseline.json
    python3 benchmarks/compare.py results/X.json --smoke  # invariants only (CI: shared runners)
    python3 benchmarks/compare.py --update-baseline       # accept latest.json as the new baseline

Invariants (gates.json) are machine-independent and always enforced. Regression checks compare
against `benchmarks/baseline.json` and are enforced only when both runs share a machine
fingerprint (CPU, cores, memory, OS, container runtime) and mode; otherwise they are reported as
"not comparable" — numbers from different machines must not gate each other.
Exit status: 0 pass, 1 gate failed, 2 usage/input error.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
GATES = HERE / "gates.json"
BASELINE = HERE / "baseline.json"
MISSING = object()


def resolve(doc, path: str):
    cur = doc
    for part in re.findall(r"[^.\[\]]+|\[[^\]]+\]", path):
        if part.startswith("["):
            conds = [c.partition("=")[::2] for c in part[1:-1].split(",")]
            if not isinstance(cur, list):
                return MISSING
            hits = [e for e in cur if isinstance(e, dict) and all(str(e.get(k)) == v for k, v in conds)]
            if len(hits) != 1:  # ambiguous selectors are an error, not "first match"
                return MISSING
            cur = hits[0]
        elif isinstance(cur, dict):
            cur = cur.get(part, MISSING)
        else:
            return MISSING
        if cur is MISSING:
            return MISSING
    return cur


def fingerprint(result: dict) -> tuple:
    e = result.get("environment", {})
    return (e.get("cpu"), e.get("logical_cores"), e.get("memory_gb"), e.get("os"), e.get("container_runtime"), result.get("smoke"))


def suite_ok(result: dict, path: str) -> bool:
    m = re.match(r"suites\.([^.]+)\.", path)
    return not m or resolve(result, f"suites.{m.group(1)}.status") == "ok"


def check_invariants(result: dict, gates: list[dict]) -> list[tuple[str, str, str]]:
    rows = []
    for g in gates:
        if not suite_ok(result, g["path"]):
            rows.append(("SKIP", g["name"], "suite not run"))
            continue
        v = resolve(result, g["path"])
        if v is MISSING or v is None:
            rows.append(("FAIL", g["name"], f"no value at {g['path']}"))
            continue
        problems = []
        if "min" in g and v < g["min"]:
            problems.append(f"{v} < {g['min']}")
        if "max" in g and v > g["max"]:
            problems.append(f"{v} > {g['max']}")
        if "equals_path" in g:
            other = resolve(result, g["equals_path"])
            if other is MISSING or v != other:
                problems.append(f"{v} != {g['equals_path']} ({other if other is not MISSING else 'missing'})")
        rows.append(("FAIL" if problems else "PASS", g["name"], "; ".join(problems) or f"{v}"))
    return rows


def check_regressions(result: dict, base: dict, gates: list[dict]) -> list[tuple[str, str, str]]:
    rows = []
    for g in gates:
        if not suite_ok(result, g["path"]) or not suite_ok(base, g["path"]):
            rows.append(("SKIP", g["name"], "suite not in both runs"))
            continue
        v, b = resolve(result, g["path"]), resolve(base, g["path"])
        if v in (MISSING, None) or b in (MISSING, None) or b == 0:
            rows.append(("SKIP", g["name"], "no comparable value"))
            continue
        change = (v - b) / b
        worse = -change if g["better"] == "higher" else change
        status = "FAIL" if worse > g["tolerance"] else "PASS"
        rows.append((status, g["name"], f"{b:g} → {v:g} ({change:+.1%}, tolerance {g['tolerance']:.0%})"))
    return rows


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("result", nargs="?", help="result file (default: results/latest.json or latest-smoke.json)")
    ap.add_argument("--baseline", default=str(BASELINE))
    ap.add_argument("--smoke", action="store_true", help="invariants only")
    ap.add_argument("--update-baseline", action="store_true")
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()

    default = HERE / "results" / ("latest-smoke.json" if a.smoke else "latest.json")
    path = Path(a.result) if a.result else default
    if not path.exists():
        print(f"no result file at {path}; run benchmarks/run.py first", file=sys.stderr)
        return 2
    result = json.loads(path.read_text())
    if a.update_baseline:
        if result.get("smoke"):
            print("refusing to use a smoke run as the baseline", file=sys.stderr)
            return 2
        failed = [k for k, v in result["suites"].items() if v["status"] != "ok"]
        if failed:
            print(f"refusing: suites failed in this run: {failed}", file=sys.stderr)
            return 2
        shutil.copyfile(path, a.baseline)
        print(f"baseline ← {path.name}")
        return 0

    gates = json.loads(GATES.read_text())
    report = {"result": path.name, "invariants": check_invariants(result, gates["invariants"]), "regressions": []}
    base_path = Path(a.baseline)
    note = ""
    if a.smoke:
        note = "smoke mode: regression checks skipped"
    elif not base_path.exists():
        note = "no baseline yet (python3 benchmarks/compare.py --update-baseline)"
    else:
        base = json.loads(base_path.read_text())
        if fingerprint(base) != fingerprint(result):
            note = "baseline recorded on a different machine/mode: regression checks not comparable"
        else:
            report["regressions"] = check_regressions(result, base, gates["regressions"])
            report["baseline"] = base_path.name
    report["note"] = note
    failed = [r for r in report["invariants"] + report["regressions"] if r[0] == "FAIL"]

    if a.json:
        print(json.dumps(report, indent=2))
    else:
        for title, rows in (("Invariants", report["invariants"]), ("Regressions", report["regressions"])):
            if rows:
                print(f"\n{title}")
                for status, name, detail in rows:
                    print(f"  {status:4}  {name:58} {detail}")
        if note:
            print(f"\nnote: {note}")
        print(f"\n{'FAILED' if failed else 'PASSED'}: {len(failed)} gate(s) failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
