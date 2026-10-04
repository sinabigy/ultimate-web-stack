"""Self-test for the regression gates: a synthetic slowdown must fail, noise within tolerance
must pass, a different machine must not be compared, a broken invariant must fail.

    python3 -m unittest benchmarks/test_compare.py
"""

from __future__ import annotations

import copy
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
COMPARE = HERE / "compare.py"


def result() -> dict:
    """Minimal complete result satisfying every gate path in gates.json."""
    http = lambda rps, p99: {"success_rps": rps, "success_rate": 1.0, "p99_ms": p99, "server_rss_mb_max": 50.0}  # noqa: E731
    return {
        "smoke": False,
        "environment": {"cpu": "Test CPU", "logical_cores": 8, "memory_gb": 16.0, "os": "TestOS", "container_runtime": "vm"},
        "suites": {
            "http": {"status": "ok", "data": {
                "plaintext": [{"concurrency": 64, **http(100_000, 1.5)}],
                "json": [{"concurrency": 64, **http(90_000, 2.0)}],
                "db": http(10_000, 15.0), "cached": http(95_000, 2.0)}},
            "auth": {"status": "ok", "data": {
                "org_runs_page": http(2_500, 40.0),
                "unauthenticated_401": {"requests": 1000, "status_codes": {"401": 1000}},
                "cross_tenant_404": {"requests": 500, "status_codes": {"404": 500}}}},
            "outbound": {"status": "ok", "data": {
                "connection_reuse": {"pooled_keepalive": {"upstream": {"connection_reuse": 0.99}}},
                "rate_limited_500rps": {"engine": {"success_rate": 1.0, "wall_s": 20.0, "upstream": {"waste_ratio": 0.02}}},
                "overloaded_capacity16": {"engine": {"success_rate": 1.0, "useful_rps": 1560.0, "upstream": {"waste_ratio": 0.002}}},
                "outage_open_loop_engine": {"amplification_during_outage": 0.5, "recovery_s": 0.5}}},
            "db": {"status": "ok", "data": {"results": [
                {"query": "session_auth_lookup", "pool_size": 32, "concurrency": 64, "ops_per_sec": 7000.0},
                {"query": "session_auth_lookup", "pool_size": 8, "concurrency": 64, "ops_per_sec": 4000.0}]}},
        },
    }


class CompareGates(unittest.TestCase):
    def run_compare(self, current: dict, baseline: dict) -> subprocess.CompletedProcess:
        with tempfile.TemporaryDirectory() as d:
            cur, base = Path(d) / "cur.json", Path(d) / "base.json"
            cur.write_text(json.dumps(current))
            base.write_text(json.dumps(baseline))
            return subprocess.run([sys.executable, str(COMPARE), str(cur), "--baseline", str(base)], text=True, capture_output=True)

    def test_identical_run_passes(self):
        r = self.run_compare(result(), result())
        self.assertEqual(r.returncode, 0, r.stdout)

    def test_synthetic_slowdown_is_flagged(self):
        slow = result()
        slow["suites"]["http"]["data"]["plaintext"][0]["success_rps"] = 70_000  # −30%
        slow["suites"]["auth"]["data"]["org_runs_page"]["p99_ms"] = 80.0  # +100%
        r = self.run_compare(slow, result())
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("FAIL  plaintext throughput", r.stdout)
        self.assertIn("FAIL  authenticated tenant page p99", r.stdout)

    def test_change_within_tolerance_passes(self):
        noisy = result()
        noisy["suites"]["http"]["data"]["plaintext"][0]["success_rps"] = 95_000  # −5%
        self.assertEqual(self.run_compare(noisy, result()).returncode, 0)

    def test_different_machine_is_not_compared(self):
        other = result()
        other["environment"]["cpu"] = "Other CPU"
        other["suites"]["http"]["data"]["plaintext"][0]["success_rps"] = 10_000
        r = self.run_compare(other, result())
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertIn("not comparable", r.stdout)

    def test_broken_invariant_fails_regardless_of_machine(self):
        bad = result()
        bad["environment"]["cpu"] = "Other CPU"
        bad["suites"]["auth"]["data"]["cross_tenant_404"]["status_codes"] = {"404": 499, "200": 1}
        r = self.run_compare(bad, result())
        self.assertEqual(r.returncode, 1, r.stdout)
        self.assertIn("FAIL  cross-tenant probes are always 404", r.stdout)

    def test_ambiguous_selector_is_not_first_match(self):
        amb = copy.deepcopy(result())
        amb["suites"]["db"]["data"]["results"].append(
            {"query": "session_auth_lookup", "pool_size": 32, "concurrency": 64, "ops_per_sec": 1.0})
        r = self.run_compare(amb, result())
        self.assertIn("SKIP  session lookup ops/s", r.stdout)


if __name__ == "__main__":
    unittest.main()
