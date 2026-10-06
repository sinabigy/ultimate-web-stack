"""Development ports of a project: infra/dev-ports.env, each overridable by the same-named
environment variable. Shared by ./dev and the scripts it runs (standard library only).

Port blocks for generated projects (scripts/create-project) are also defined here, so the generator
and the tests agree on the layout.
"""
from __future__ import annotations

import hashlib
import os
import socket
import subprocess
from pathlib import Path

PORTS_FILE = "infra/dev-ports.env"
# A generated project's block: BLOCK_FIRST + BLOCK_SIZE * n, below the ephemeral ranges of macOS
# (49152+) and Linux (32768+) so the OS never hands these ports to outgoing connections.
BLOCK_FIRST, BLOCK_SIZE, BLOCK_COUNT = 20000, 32, 300


def read_file(root: Path) -> dict[str, int]:
    """The ports declared in the project's file, in file order."""
    out: dict[str, int] = {}
    for line in (root / PORTS_FILE).read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            k, v = line.split("=", 1)
            out[k.strip()] = int(v.strip())
    return out


def load(root: Path, env: dict[str, str] | None = None) -> dict[str, int]:
    """Effective ports: the file, overridden by environment variables of the same name."""
    env = os.environ if env is None else env
    ports = read_file(root)
    for k in ports:
        if env.get(k):
            ports[k] = int(env[k])
    return ports


def default_base(slug: str) -> int:
    """Deterministic block for a project name, so regenerating a project keeps its ports."""
    n = int.from_bytes(hashlib.sha256(slug.encode()).digest()[:4], "big") % BLOCK_COUNT
    return BLOCK_FIRST + BLOCK_SIZE * n


def block(names: list[str], base: int) -> dict[str, int]:
    """Assign consecutive ports from `base` in the order the file declares them."""
    if len(names) > BLOCK_SIZE:
        raise ValueError(f"{len(names)} ports do not fit a block of {BLOCK_SIZE}")
    if not 1024 <= base <= 65535 - BLOCK_SIZE:
        raise ValueError(f"port base {base} must be between 1024 and {65535 - BLOCK_SIZE}")
    return {name: base + i for i, name in enumerate(names)}


def in_use(port: int) -> bool:
    """Something accepts connections on localhost:port (IPv4 or IPv6)."""
    for host in ("127.0.0.1", "::1"):
        try:
            with socket.create_connection((host, port), timeout=0.3):
                return True
        except OSError:
            pass
    return False


def _out(cmd: list[str]) -> str:
    try:
        return subprocess.run(cmd, capture_output=True, text=True).stdout
    except FileNotFoundError:
        return ""


def holder(port: int) -> str:
    """Best-effort description of what listens on a port, for error messages."""
    for line in _out(["docker", "ps", "--format", "{{.Names}}\t{{.Ports}}"]).splitlines():
        name, _, ports = line.partition("\t")
        if any(p.split("->")[0].rsplit(":", 1)[-1] == str(port) for p in ports.split(", ") if "->" in p):
            return f"container {name}"
    fields = dict((ln[0], ln[1:]) for ln in _out(["lsof", "-nP", f"-iTCP:{port}", "-sTCP:LISTEN", "-Fpc"]).splitlines()
                  if ln[:1] in ("p", "c"))
    if "p" not in fields:
        return ""
    cwd = _out(["lsof", "-a", "-p", fields["p"], "-d", "cwd", "-Fn"])
    where = next((ln[1:] for ln in cwd.splitlines() if ln.startswith("n")), "")
    return f"pid {fields['p']} ({fields.get('c', '?')})" + (f" in {where}" if where else "")


def listener_pids(port: int) -> set[int]:
    """PIDs listening on a port (empty when lsof is unavailable)."""
    return {int(p) for p in _out(["lsof", "-nP", f"-iTCP:{port}", "-sTCP:LISTEN", "-t"]).split()}
