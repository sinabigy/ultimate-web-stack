"""Shared helpers for the ai-* tools.

Guaranteed runtime: Python >= 3.9 standard library. Everything else (git, ctags,
the `jsonschema` package, `tomllib`) is optional and detected at runtime.
Set AI_TOOLS_DISABLE=git,ctags,jsonschema,tomllib to force the degraded paths.
"""
from __future__ import annotations

import datetime as _dt
import fnmatch
import hashlib
import json
import os
import re
import shutil
import subprocess
from pathlib import Path

TEMPLATE_VERSION = "1.2.0"
AI_PROTOCOL_VERSION = "1.2.0"
MAPPER_VERSION = "1.1.0"

TASK_STATUSES = ("queue", "active", "blocked", "completed")
TASK_ID_RE = re.compile(r"^T-\d{4,}$")

TOOLS_DIR = Path(__file__).resolve().parent
TEMPLATE_ROOT = TOOLS_DIR.parent


# ---------------------------------------------------------------- environment

def disabled(feature: str) -> bool:
    raw = os.environ.get("AI_TOOLS_DISABLE", "")
    return feature in {p.strip() for p in raw.split(",") if p.strip()}


def now_iso() -> str:
    return _dt.datetime.now(_dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def find_root(explicit: str | None = None) -> Path:
    """Repo root: --root if given, else nearest ancestor of cwd with .ai/config/project.json,
    else the repo containing these tools."""
    if explicit:
        return Path(explicit).resolve()
    here = Path.cwd().resolve()
    for p in (here, *here.parents):
        if (p / ".ai" / "config" / "project.json").is_file():
            return p
    return TEMPLATE_ROOT


def safe_path(root: Path, rel: str | Path) -> Path:
    """Resolve rel under root; refuse anything that escapes root (path traversal)."""
    root = root.resolve()
    p = (root / rel).resolve()
    if p != root and root not in p.parents:
        raise ValueError(f"path escapes repository root: {rel}")
    return p


def load_json(path: Path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def dump_json(root: Path, rel: str | Path, data) -> Path:
    p = safe_path(root, rel)
    p.parent.mkdir(parents=True, exist_ok=True)
    tmp = p.with_name(p.name + ".tmp")
    tmp.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    os.replace(tmp, p)
    return p


def run(cmd: list[str], cwd: Path) -> subprocess.CompletedProcess | None:
    try:
        return subprocess.run(cmd, cwd=str(cwd), capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.SubprocessError):
        return None


# ---------------------------------------------------------------- optional deps

def have_git(root: Path) -> bool:
    if disabled("git") or not shutil.which("git"):
        return False
    r = run(["git", "rev-parse", "--is-inside-work-tree"], root)
    return bool(r and r.returncode == 0 and r.stdout.strip() == "true")


def have_ctags() -> bool:
    if disabled("ctags") or not shutil.which("ctags"):
        return False
    r = run(["ctags", "--version"], Path.cwd())
    return bool(r and r.returncode == 0 and "Universal Ctags" in r.stdout)


def jsonschema_module():
    if disabled("jsonschema"):
        return None
    try:
        import jsonschema  # type: ignore
        return jsonschema
    except ImportError:
        return None


def tomllib_module():
    if disabled("tomllib"):
        return None
    try:
        import tomllib  # type: ignore  # Python 3.11+
        return tomllib
    except ImportError:
        return None


# ---------------------------------------------------------------- validation
# Baseline validator: a documented SUBSET of JSON Schema 2020-12.
# Keywords outside the subset are never silently ignored; they are reported as
# "not checked". Pure annotations are skipped because they never constrain data.

BASELINE_KEYWORDS = {
    "type", "required", "properties", "additionalProperties", "propertyNames",
    "enum", "const", "pattern", "items", "minItems", "maxItems", "uniqueItems",
    "minLength", "maxLength", "minimum", "maximum", "not", "anyOf", "$ref", "$defs",
}
ANNOTATIONS = {
    "$schema", "$id", "$comment", "title", "description", "default", "examples",
    "deprecated", "readOnly", "writeOnly", "format",  # format is annotation-only by default in 2020-12
}

_TYPES = {
    "object": lambda v: isinstance(v, dict),
    "array": lambda v: isinstance(v, list),
    "string": lambda v: isinstance(v, str),
    "boolean": lambda v: isinstance(v, bool),
    "null": lambda v: v is None,
    "integer": lambda v: isinstance(v, int) and not isinstance(v, bool),
    "number": lambda v: isinstance(v, (int, float)) and not isinstance(v, bool),
}


class Validation:
    def __init__(self, level: str):
        self.level = level
        self.errors: list[str] = []
        self.unchecked: list[str] = []

    @property
    def ok(self) -> bool:
        return not self.errors


def _resolve_ref(ref: str, root_schema: dict) -> dict:
    if not ref.startswith("#/"):
        raise ValueError(f"only local $ref is supported by the baseline validator: {ref}")
    node = root_schema
    for part in ref[2:].split("/"):
        node = node[part.replace("~1", "/").replace("~0", "~")]
    return node


def _baseline(inst, schema, root_schema, path: str, out: Validation) -> None:
    if schema is True or schema == {}:
        return
    if schema is False:
        out.errors.append(f"{path or '/'}: no value allowed here")
        return
    for kw in schema:
        if kw not in BASELINE_KEYWORDS and kw not in ANNOTATIONS:
            msg = f"not checked: '{kw}' at {path or '/'}"
            if msg not in out.unchecked:
                out.unchecked.append(msg)
    if "$ref" in schema:
        _baseline(inst, _resolve_ref(schema["$ref"], root_schema), root_schema, path, out)
    p = path or "/"
    if "type" in schema:
        types = schema["type"] if isinstance(schema["type"], list) else [schema["type"]]
        if not any(_TYPES[t](inst) for t in types):
            out.errors.append(f"{p}: expected type {'|'.join(types)}, got {type(inst).__name__}")
            return
    if "enum" in schema and inst not in schema["enum"]:
        out.errors.append(f"{p}: {inst!r} not one of {schema['enum']}")
    if "const" in schema and inst != schema["const"]:
        out.errors.append(f"{p}: must equal {schema['const']!r}")
    if "not" in schema:
        probe = Validation("baseline")
        _baseline(inst, schema["not"], root_schema, path, probe)
        out.unchecked.extend(u for u in probe.unchecked if u not in out.unchecked)
        if probe.ok:
            out.errors.append(f"{p}: value matches a forbidden pattern ({inst!r})")
    if "anyOf" in schema:
        probes = []
        for sub in schema["anyOf"]:
            probe = Validation("baseline")
            _baseline(inst, sub, root_schema, path, probe)
            probes.append(probe)
        if not any(pr.ok for pr in probes):
            out.errors.append(f"{p}: does not match any allowed form")
    if isinstance(inst, str):
        if "minLength" in schema and len(inst) < schema["minLength"]:
            out.errors.append(f"{p}: shorter than {schema['minLength']}")
        if "maxLength" in schema and len(inst) > schema["maxLength"]:
            out.errors.append(f"{p}: longer than {schema['maxLength']}")
        if "pattern" in schema and not re.search(schema["pattern"], inst):
            out.errors.append(f"{p}: {inst!r} does not match /{schema['pattern']}/")
    if _TYPES["number"](inst):
        if "minimum" in schema and inst < schema["minimum"]:
            out.errors.append(f"{p}: below minimum {schema['minimum']}")
        if "maximum" in schema and inst > schema["maximum"]:
            out.errors.append(f"{p}: above maximum {schema['maximum']}")
    if isinstance(inst, list):
        if "minItems" in schema and len(inst) < schema["minItems"]:
            out.errors.append(f"{p}: needs at least {schema['minItems']} item(s)")
        if "maxItems" in schema and len(inst) > schema["maxItems"]:
            out.errors.append(f"{p}: allows at most {schema['maxItems']} item(s)")
        if schema.get("uniqueItems"):
            seen = [json.dumps(i, sort_keys=True) for i in inst]
            if len(seen) != len(set(seen)):
                out.errors.append(f"{p}: items must be unique")
        if isinstance(schema.get("items"), (dict, bool)):
            for i, item in enumerate(inst):
                _baseline(item, schema["items"], root_schema, f"{path}/{i}", out)
    if isinstance(inst, dict):
        for req in schema.get("required", []):
            if req not in inst:
                out.errors.append(f"{p}: missing required property '{req}'")
        props = schema.get("properties", {})
        for k, v in inst.items():
            if "propertyNames" in schema:
                _baseline(k, schema["propertyNames"], root_schema, f"{path}/{k}(name)", out)
            if k in props:
                _baseline(v, props[k], root_schema, f"{path}/{k}", out)
            elif "additionalProperties" in schema:
                ap = schema["additionalProperties"]
                if ap is False:
                    out.errors.append(f"{p}: property '{k}' is not allowed")
                else:
                    _baseline(v, ap, root_schema, f"{path}/{k}", out)


def validation_level() -> str:
    js = jsonschema_module()
    if js is not None:
        try:
            from importlib.metadata import version
            ver = version("jsonschema")
        except Exception:
            ver = "?"
        return f"full (jsonschema {ver})"
    return "baseline (documented subset; see docs/protocol/validation.md)"


def validate(instance, schema: dict) -> Validation:
    js = jsonschema_module()
    if js is not None:
        out = Validation(validation_level())
        validator_cls = getattr(js, "Draft202012Validator", None)
        if validator_cls is None:  # very old jsonschema: fall back honestly
            js = None
        else:
            for err in validator_cls(schema).iter_errors(instance):
                loc = "/" + "/".join(str(x) for x in err.absolute_path)
                out.errors.append(f"{loc}: {err.message}")
            return out
    out = Validation(validation_level())
    _baseline(instance, schema, schema, "", out)
    return out


def load_schema(root: Path, name: str) -> dict:
    return load_json(root / ".ai" / "schemas" / f"{name}.schema.json")


# ---------------------------------------------------------------- tasks

def task_files(root: Path) -> list[tuple[str, Path]]:
    out = []
    for status in TASK_STATUSES:
        d = root / ".ai" / "tasks" / status
        if d.is_dir():
            out.extend((status, p) for p in sorted(d.glob("T-*.json")))
    return out


def slugify(text: str, limit: int = 40) -> str:
    s = re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")
    if len(s) > limit:
        cut = s[:limit]
        s = cut.rsplit("-", 1)[0] if "-" in cut else cut
    return s or "task"


# ---------------------------------------------------------------- repository listing (bounded)

DEFAULT_EXCLUDE_DIRS = {
    ".git", ".hg", ".svn", ".ai", "node_modules", ".venv", "venv", "__pycache__",
    ".mypy_cache", ".pytest_cache", ".ruff_cache", ".tox", ".nox", "dist", "build",
    "target", ".next", ".nuxt", ".turbo", ".cache", "coverage", ".gradle", ".idea",
    ".vscode", "vendor", "Pods", ".terraform", ".parcel-cache", "bower_components",
}
# OS/editor metadata files: never project content (e.g. macOS Finder writes .DS_Store into every folder it opens)
DEFAULT_EXCLUDE_FILES = {".DS_Store", "Thumbs.db", "desktop.ini", "Icon\r"}
BINARY_EXT = {
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".bmp", ".pdf", ".zip", ".gz",
    ".tgz", ".tar", ".7z", ".rar", ".jar", ".class", ".so", ".dylib", ".dll", ".exe",
    ".bin", ".o", ".a", ".wasm", ".mp3", ".mp4", ".mov", ".wav", ".ogg", ".ttf", ".otf",
    ".woff", ".woff2", ".db", ".sqlite", ".sqlite3", ".pyc", ".psd", ".blend", ".fbx",
}


def _simple_gitignore(root: Path) -> list[str]:
    gi = root / ".gitignore"
    if not gi.is_file():
        return []
    pats = []
    for line in gi.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if line and not line.startswith("#") and not line.startswith("!"):
            pats.append(line)
    return pats


def _ignored(rel: str, pats: list[str]) -> bool:
    name = rel.rsplit("/", 1)[-1]
    for pat in pats:
        p = pat.rstrip("/").lstrip("/")
        if fnmatch.fnmatch(rel, p) or fnmatch.fnmatch(name, p) or rel.startswith(p + "/"):
            return True
    return False


def list_files(root: Path, map_cfg: dict | None = None) -> dict:
    """Bounded file listing. Records metadata only; never reads content.
    Returns {method, files:[(rel,size,mtime_ns)], truncated, skipped_dirs}."""
    cfg = map_cfg or {}
    max_files = int(cfg.get("max_files", 20000))
    extra_ex = list(cfg.get("exclude", []))
    include = list(cfg.get("include", []))
    files: list[tuple[str, int, int]] = []
    truncated = False

    def wanted(rel: str) -> bool:
        parts = rel.split("/")
        if parts[-1] in DEFAULT_EXCLUDE_FILES or parts[-1].startswith("._"):
            return False
        if any(part in DEFAULT_EXCLUDE_DIRS for part in parts[:-1]):
            return False
        if extra_ex and _ignored(rel, extra_ex):
            return False
        if include and not any(fnmatch.fnmatch(rel, pat) for pat in include):
            return False
        return True

    def add(rel: str) -> bool:
        nonlocal truncated
        if len(files) >= max_files:
            truncated = True
            return False
        try:
            st = os.lstat(root / rel)
        except OSError:
            return True
        if not os.path.isfile(root / rel) or os.path.islink(root / rel):
            return True
        files.append((rel, st.st_size, st.st_mtime_ns))
        return True

    if have_git(root):
        method = "git ls-files -co --exclude-standard"
        r = run(["git", "ls-files", "-co", "--exclude-standard", "-z"], root)
        for rel in sorted(x for x in (r.stdout.split("\0") if r else []) if x):
            if wanted(rel) and not add(rel):
                break
    else:
        method = "directory walk (default excludes + simple root .gitignore patterns)"
        pats = _simple_gitignore(root)
        stop = False
        for dirpath, dirnames, filenames in os.walk(root):
            rel_dir = os.path.relpath(dirpath, root).replace(os.sep, "/")
            rel_dir = "" if rel_dir == "." else rel_dir
            dirnames[:] = sorted(
                d for d in dirnames
                if d not in DEFAULT_EXCLUDE_DIRS
                and not _ignored(f"{rel_dir}/{d}".lstrip("/"), pats)
                and not os.path.islink(os.path.join(dirpath, d))
            )
            for fn in sorted(filenames):
                rel = f"{rel_dir}/{fn}".lstrip("/")
                if _ignored(rel, pats) or not wanted(rel):
                    continue
                if not add(rel):
                    stop = True
                    break
            if stop:
                break
    return {"method": method, "files": files, "truncated": truncated}


def fingerprint(files: list[tuple[str, int, int]]) -> str:
    h = hashlib.sha256()
    for rel, size, mtime in files:
        h.update(f"{rel}\0{size}\0{mtime}\n".encode())
    return "sha256:" + h.hexdigest()[:32]


def git_head(root: Path) -> tuple[str | None, bool | None]:
    if not have_git(root):
        return None, None
    r = run(["git", "rev-parse", "HEAD"], root)
    head = r.stdout.strip() if r and r.returncode == 0 else None
    s = run(["git", "status", "--porcelain", "--untracked-files=normal"], root)
    dirty = bool(s and s.stdout.strip()) if s and s.returncode == 0 else None
    return head, dirty
