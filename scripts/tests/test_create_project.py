"""Generator tests: generated projects match their flags and pass the protocol check.

    python3 -m unittest discover -s scripts/tests
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GEN = ROOT / "scripts" / "create-project"


class Generated:
    def __init__(self, path: Path):
        self.path = path
        self.project = json.loads((path / ".ai/config/project.json").read_text())
        self.arch = self.project["architecture"]
        self.config = tomllib.loads((path / "backend/config/app.toml").read_text())
        self.commands = {c["id"] for c in self.project["validation"]["commands"]}

    def enabled(self, module: str) -> bool:
        return self.arch["modules"][module]["enabled"]


class CreateProjectTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp(prefix="create-project-test-"))

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def gen(self, name: str, *flags: str, ok: bool = True) -> subprocess.CompletedProcess:
        r = subprocess.run([sys.executable, str(GEN), str(self.tmp / name), "--name", name.title(), *flags],
                           capture_output=True, text=True, timeout=300)
        if ok:
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        return r

    def test_core_b2b_default(self):
        self.gen("core-app", "--profile", "core", "--no-start")
        g = Generated(self.tmp / "core-app")
        self.assertEqual(g.arch["profile"], "core")
        self.assertEqual(g.arch["components"]["auth_profile"], "b2b")
        self.assertTrue(g.enabled("organizations") and g.enabled("admin"), "auth/admin foundation on by default")
        for m in ("messaging_nats", "analytics_clickhouse", "observability"):
            self.assertFalse(g.enabled(m), m)
        self.assertEqual(g.config["tenancy"]["organizations"], True)
        self.assertEqual(g.config["admin"]["enabled"], True)
        self.assertEqual(g.config["cache"]["backend"], "memory")
        self.assertEqual(g.config["authorization"]["engine"], "rbac")
        # module-specific validation only for selected modules; gateway removed with its job
        self.assertTrue({"rust-test", "e2e", "infra-verify"} <= g.commands)
        self.assertFalse({"rust-test-nats", "rust-test-clickhouse", "rust-test-cache", "gateway-check"} & g.commands)
        self.assertFalse((g.path / "backend/gateway").exists())
        self.assertNotIn("\n  gateway:", (g.path / ".github/workflows/ci.yml").read_text())
        self.assertIn('BRAND = "Core-App"', (g.path / "frontend/src/brand.ts").read_text())
        # fresh protocol instance with provenance, committed, clean
        self.assertEqual(g.project["instance"]["origin"], "derived")
        self.assertEqual(g.arch["origin"]["name"], "ultimate-web-stack-blueprint")
        tasks = list((g.path / ".ai/tasks").rglob("T-*.json"))
        self.assertEqual([t.name[:6] for t in tasks], ["T-0001"])
        log = subprocess.run(["git", "log", "--oneline"], cwd=g.path, capture_output=True, text=True).stdout
        self.assertEqual(len(log.strip().splitlines()), 1)
        self.assertEqual(subprocess.run(["git", "status", "--porcelain"], cwd=g.path, capture_output=True, text=True).stdout, "")
        check = subprocess.run([sys.executable, "tools/ai-check"], cwd=g.path, capture_output=True, text=True)
        self.assertEqual(check.returncode, 0, check.stdout)
        self.assertFalse(any(p.name in (".env", ".env.zitadel") for p in g.path.iterdir()), "no secrets copied")

    def test_basic_without_admin_is_single_tenant(self):
        self.gen("basic-app", "--auth", "basic", "--no-admin", "--no-git")
        g = Generated(self.tmp / "basic-app")
        self.assertFalse(g.enabled("organizations"))
        self.assertFalse(g.enabled("admin"))
        self.assertEqual(g.arch["components"]["tenancy"], "personal-workspace")
        self.assertEqual(g.config["auth"]["methods"]["passkey"], False)
        self.assertEqual(g.config["tenancy"]["organizations"], False)

    def test_full_profile_with_cedar_and_gateway(self):
        self.gen("full-app", "--profile", "full", "--auth", "enterprise", "--cedar", "--with-gateway", "--no-git")
        g = Generated(self.tmp / "full-app")
        self.assertEqual(g.arch["profile"], "full")
        for m in ("cache", "messaging_nats", "analytics_clickhouse", "observability", "cedar"):
            self.assertTrue(g.enabled(m), m)
        self.assertEqual(g.config["authorization"]["engine"], "cedar")
        self.assertEqual(g.config["auth"]["methods"]["enterprise_sso"], True)
        self.assertEqual((g.config["messaging"]["enabled"], g.config["analytics"]["enabled"]), (True, True))
        self.assertTrue({"rust-test-nats", "rust-test-clickhouse", "rust-test-cache", "gateway-check"} <= g.commands)
        self.assertTrue((g.path / "backend/gateway/src/main.rs").exists())

    def test_invalid_combinations_are_refused(self):
        r = self.gen("bad1", "--auth", "b2b", "--no-organizations", "--no-git", ok=False)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("implies organizations", r.stderr)
        r = self.gen("bad2", "--modules", "kafka", "--no-git", ok=False)
        self.assertIn("unknown modules", r.stderr)
        (self.tmp / "occupied").mkdir()
        (self.tmp / "occupied" / "file").write_text("x")
        r = self.gen("occupied", "--no-git", ok=False)
        self.assertIn("not empty", r.stderr)
        r = subprocess.run([sys.executable, str(GEN), str(ROOT / "nested"), "--name", "X"], capture_output=True, text=True)
        self.assertIn("outside the blueprint", r.stderr)


if __name__ == "__main__":
    unittest.main()
