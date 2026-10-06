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
sys.path.insert(0, str(ROOT / "scripts"))
import dev_ports  # noqa: E402


class Generated:
    def __init__(self, path: Path):
        self.path = path
        self.project = json.loads((path / ".ai/config/project.json").read_text())
        self.arch = self.project["architecture"]
        self.config = tomllib.loads((path / "backend/config/app.toml").read_text())
        self.commands = {c["id"] for c in self.project["validation"]["commands"]}
        self.ports = dev_ports.read_file(path)

    def enabled(self, module: str) -> bool:
        return self.arch["modules"][module]["enabled"]


def broken_links(root: Path) -> list[str]:
    """Relative markdown links (files and directories) that do not resolve, in tracked .md files."""
    import re
    out = []
    files = subprocess.run(["git", "ls-files", "*.md"], cwd=root, capture_output=True, text=True).stdout.split()
    for rel in files:
        text = (root / rel).read_text(errors="replace")
        text = re.sub(r"```.*?```", "", text, flags=re.S)  # ignore code blocks
        for target in re.findall(r"\]\(([^)\s]+)\)", text):
            if re.match(r"^(https?:|mailto:|#)", target) or "«" in target:
                continue
            path = target.split("#")[0]
            if path and not (root / rel).parent.joinpath(path).exists():
                out.append(f"{rel} -> {target}")
    return out


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
        self.assertEqual(g.arch["modules"]["cache"]["adapter"], "memory")
        self.assertEqual(g.config["authorization"]["engine"], "rbac")
        # module-specific validation only for selected modules; gateway removed with its job
        self.assertTrue({"rust-test", "e2e", "infra-verify"} <= g.commands)
        self.assertFalse({"rust-test-nats", "rust-test-clickhouse", "rust-test-cache", "gateway-check"} & g.commands)
        self.assertFalse((g.path / "backend/gateway").exists())
        # unselected modules are not compiled: no optional cargo features by default
        for app in ("server", "worker", "bench"):
            self.assertIn("\ndefault = []\n", (g.path / f"backend/apps/{app}/Cargo.toml").read_text(), app)
        self.assertFalse((g.path / "scripts/create-project").exists(), "the generator is blueprint-only")
        self.assertNotIn("create-project", (g.path / "docs/README.md").read_text(), "no links to blueprint-only tools")
        # The blueprint's open-source machinery stays in the blueprint; license notices travel.
        for f in ("CONTRIBUTING.md", "CODE_OF_CONDUCT.md", "SECURITY.md", "CHANGELOG.md", "LICENSE-MIT", "site",
                  ".github/FUNDING.yml", ".github/ISSUE_TEMPLATE", ".github/workflows/release.yml",
                  "scripts/release-audit.sh", "docs/FAQ.md", "docs/WHY.md"):
            self.assertFalse((g.path / f).exists(), f)
        for f in ("LICENSE-MIT", "LICENSE-APACHE"):
            self.assertTrue((g.path / "third_party_licenses/ultimate-web-stack" / f).is_file(), f)
        self.assertTrue((g.path / ".github/workflows/ci.yml").is_file())
        self.assertEqual(broken_links(g.path), [], "documentation links in the generated project resolve")
        self.assertIn("docs/MODULES.md", (g.path / "README.md").read_text())
        ci = (g.path / ".github/workflows/ci.yml").read_text()
        self.assertNotIn("\n  gateway:", ci)
        # CI starts no service and runs no command for unselected modules.
        for absent in ("redis:", "dragonfly:", "clickhouse:", "NATS with JetStream", "rust-test-cache",
                       "rust-test-nats", "rust-test-clickhouse", "rust-test-cedar"):
            self.assertNotIn(absent, ci, absent)
        self.assertIn("postgres:", ci)
        self.assertNotIn("rust-test-cedar", g.commands)
        unconfirmed = [c["id"] for c in g.project["validation"]["commands"] if not c.get("confirmed")]
        self.assertEqual(unconfirmed, [], "no inferred placeholder commands")
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
        # A selected Redis backend must come with a usable (credential-free) development address.
        self.assertEqual(g.config["cache"]["redis_url"], f"redis://127.0.0.1:{g.ports['DEV_REDIS_PORT']}")
        self.assertEqual(g.arch["modules"]["cache"]["adapter"], "redis", "./dev up starts Redis only for this adapter")
        ci = (g.path / ".github/workflows/ci.yml").read_text()
        for present in ("redis:", "clickhouse:", "NATS with JetStream", "--only rust-test-cache", "--only rust-test-nats",
                        "--only rust-test-clickhouse", "--only rust-test-cedar"):
            self.assertIn(present, ci, present)
        self.assertNotIn("dragonfly:", ci)
        self.assertIn("rust-test-cedar", g.commands)
        check = subprocess.run(["bash", "-n", str(g.path / "infra/docker/smoke.sh")], capture_output=True, text=True)
        self.assertEqual(check.returncode, 0, check.stderr)
        self.assertTrue((g.path / "backend/gateway/src/main.rs").exists())
        server = (g.path / "backend/apps/server/Cargo.toml").read_text()
        self.assertIn('default = ["redis", "nats", "clickhouse", "cedar"]', server)

    def test_each_project_gets_its_own_ports(self):
        import re
        blueprint = dev_ports.read_file(ROOT)
        self.gen("ports-a", "--no-git")
        self.gen("ports-b", "--profile", "full", "--port-base", "24000", "--no-git")
        a, b = Generated(self.tmp / "ports-a"), Generated(self.tmp / "ports-b")
        # Same names as the blueprint, consecutive ports from the project's base, no overlaps.
        self.assertEqual(list(a.ports), list(blueprint))
        self.assertEqual(a.ports, dev_ports.block(list(blueprint), dev_ports.default_base("ports-a")))
        self.assertEqual(b.ports, dev_ports.block(list(blueprint), 24000))
        self.assertFalse(set(a.ports.values()) & set(b.ports.values()))
        self.assertFalse(set(a.ports.values()) & set(blueprint.values()), "generated projects run next to the blueprint")
        for g in (a, b):
            # Files used without ./dev (plain compose, tools/ai-validate) default to the project's ports.
            for rel in ("infra/docker/compose.yaml", ".ai/config/project.json"):
                defaults = re.findall(r"\$\{(DEV_[A-Z0-9_]+_PORT):-(\d+)\}", (g.path / rel).read_text())
                self.assertTrue(defaults, rel)
                self.assertEqual({(k, int(v)) for k, v in defaults}, {(k, g.ports[k]) for k, _ in defaults}, rel)
            prom = (g.path / "infra/docker/observability/prometheus.yml").read_text()
            self.assertIn(f"host.docker.internal:{g.ports['DEV_API_PORT']}", prom)
            self.assertIn(f"host.docker.internal:{g.ports['DEV_WORKER_PORT']}", prom)
            self.assertIn(f"http://localhost:{g.ports['DEV_WEB_PORT']}", (g.path / "README.md").read_text())
            # Without the caller's DEV_*_PORT overrides (./dev check may run with some set).
            clean = {k: v for k, v in __import__("os").environ.items() if not k.startswith("DEV_")}
            listed = subprocess.run([str(g.path / "dev"), "ports", "--json", "--plain"], capture_output=True, text=True, env=clean)
            self.assertEqual(json.loads(listed.stdout), g.ports, listed.stderr)
        self.assertEqual(b.config["cache"]["redis_url"], f"redis://127.0.0.1:{b.ports['DEV_REDIS_PORT']}")
        self.assertIn("--port-base 24000", (b.path / "README.md").read_text(), "an explicit block is part of the replay command")
        self.assertNotIn("--port-base", (a.path / "README.md").read_text())
        r = self.gen("ports-bad", "--port-base", "65530", "--no-git", ok=False)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("--port-base", r.stderr)
        self.assertFalse((self.tmp / "ports-bad").exists())

    def test_port_blocks_and_overrides(self):
        names = list(dev_ports.read_file(ROOT))
        self.assertLessEqual(len(names), dev_ports.BLOCK_SIZE)
        bases = {dev_ports.default_base(n) for n in ("alpha", "beta", "gamma", "my-cool-app-2")}
        for base in bases:
            self.assertEqual((base - dev_ports.BLOCK_FIRST) % dev_ports.BLOCK_SIZE, 0)
            self.assertLess(base + dev_ports.BLOCK_SIZE, 32768, "below the Linux ephemeral range")
        self.assertEqual(dev_ports.default_base("alpha"), dev_ports.default_base("alpha"), "deterministic")
        self.assertEqual(len(bases), 4)
        loaded = dev_ports.load(ROOT, {"DEV_PG_PORT": "61000", "UNRELATED": "1"})
        self.assertEqual(loaded["DEV_PG_PORT"], 61000)
        self.assertEqual(loaded["DEV_API_PORT"], dev_ports.read_file(ROOT)["DEV_API_PORT"])
        self.assertNotIn("UNRELATED", loaded)

    def test_cli_names_noninteractive_failure_cleanup_and_portability(self):
        # Help works and documents the profiles.
        h = subprocess.run([sys.executable, str(GEN), "--help"], capture_output=True, text=True)
        self.assertEqual(h.returncode, 0)
        self.assertIn("--profile", h.stdout)
        # Non-interactive without a name: a clear error, nothing created.
        r = subprocess.run([sys.executable, str(GEN), str(self.tmp / "noname")], capture_output=True, text=True,
                           stdin=subprocess.DEVNULL)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("--name and DEST are required", r.stderr)
        self.assertFalse((self.tmp / "noname").exists())
        # A failure after files were copied removes the partial destination.
        env = {**__import__("os").environ, "CREATE_PROJECT_TEST_FAIL": "after-copy"}
        r = subprocess.run([sys.executable, str(GEN), str(self.tmp / "broken"), "--name", "Broken", "--no-git"],
                           capture_output=True, text=True, env=env)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("removed the partial", r.stderr)
        self.assertFalse((self.tmp / "broken").exists())
        # Names with spaces and hyphens become a valid slug everywhere.
        r = subprocess.run([sys.executable, str(GEN), str(self.tmp / "spaced dir"), "--name", "My Cool-App 2",
                            "--no-start"], capture_output=True, text=True, timeout=300)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        g = Generated(self.tmp / "spaced dir")
        self.assertIn("COMPOSE_PROJECT_NAME:-my-cool-app-2}", (g.path / "infra/docker/compose.yaml").read_text())
        self.assertIn('"name": "my-cool-app-2-frontend"', (g.path / "frontend/package.json").read_text())
        self.assertIn('BRAND = "My Cool-App 2"', (g.path / "frontend/src/brand.ts").read_text())
        # No source-machine paths: nothing points back at the blueprint or the template checkout.
        tracked = subprocess.run(["git", "ls-files"], cwd=g.path, capture_output=True, text=True).stdout.split()
        leaks = []
        for rel in tracked:
            try:
                text = (g.path / rel).read_text()
            except (UnicodeDecodeError, IsADirectoryError):
                continue
            for needle in (str(ROOT), str(Path.home()), "~/Coding/"):
                if needle in text:
                    leaks.append(f"{rel}: {needle}")
        self.assertEqual(leaks, [], "generated project references the source machine")

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
