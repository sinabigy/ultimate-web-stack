"""Release-hygiene tests (blueprint only): the launch-placeholder guard has one pattern source.

    python3 -m unittest discover -s scripts/tests
"""

from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PATTERNS = ROOT / "scripts" / "launch-placeholders.txt"


def grep(paths: list[str]) -> set[str]:
    """Files among `paths` that contain a launch placeholder (same command as the audit)."""
    if not paths:
        return set()
    r = subprocess.run(["grep", "-IlEf", str(PATTERNS), *paths], cwd=ROOT, capture_output=True, text=True)
    return set(r.stdout.split())


class ReleaseHygieneTests(unittest.TestCase):
    def test_only_the_pattern_file_spells_out_placeholders(self):
        # A guard that spells the placeholders out becomes a false positive for the audit itself
        # (it happened with the Pages workflow), so every consumer reads the shared file instead.
        tracked = subprocess.run(["git", "ls-files"], cwd=ROOT, capture_output=True, text=True).stdout.split()
        self.assertEqual(grep(tracked) - {"scripts/launch-placeholders.txt"}, set())

    def test_guard_still_detects_placeholders(self):
        with tempfile.TemporaryDirectory() as d:
            for i, line in enumerate(PATTERNS.read_text().split()):
                page = Path(d) / f"page{i}.html"
                page.write_text(f"<a href='https://github.com/{line}'>x</a>")
                self.assertEqual(grep([str(page)]), {str(page)}, line)
        self.assertEqual(grep(["site/index.html"]), set(), "the published site has no placeholders")

    def test_pages_workflow_and_audit_use_the_shared_patterns(self):
        for rel in (".github/workflows/pages.yml", "scripts/release-audit.sh"):
            self.assertIn("scripts/launch-placeholders.txt", (ROOT / rel).read_text(), rel)


if __name__ == "__main__":
    unittest.main()
