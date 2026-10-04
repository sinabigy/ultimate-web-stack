# Context engineering

Context is a scarce resource. Its size, cost and latency all grow with what you load, and irrelevant context lowers accuracy.

## Progressive disclosure
The layers and their load order are defined in `.ai/config/context.json`.

**Always loaded:** protocol → project config → current task.

**On demand, in this order:**
1. Repository map.
2. Constraints and architecture.
3. The exact implementation.
4. Callers, dependencies and tests.
5. Decisions, lessons and git history.
6. These docs.

Before loading anything, answer: *what specific uncertainty will these tokens resolve?* If you can't name one, don't load it.

## Navigation
Prefer deterministic discovery: `rg`, symbol search, LSP/AST, compilers, test runners, `git log -S`, package metadata. Reason after the facts are in, not instead of finding them.

Good: `symbol → implementation → caller/dependency → edit`.
Bad: `read repo → speculate → search randomly → reread repo`.

## The repository map
`tools/ai-map` writes `.ai/map/repo.json`, `dependencies.json` and, with Universal Ctags, `symbols.json`. These are generated evidence and gitignored. Each one carries `meta` with:
- mapper version
- timestamp
- git HEAD
- dirty flag
- file fingerprint
- method
- unavailable optional tools

`ai-check` warns when the map is missing or stale. Never treat the map as truth over the source.

The mapper is bounded:
- It respects `.gitignore` (fully with git, a simple subset without).
- It excludes VCS, dependency, build, cache and binary paths.
- It caps the file count.
- It reads content only for manifests below a size cap.

Tune it with `map.include` and `map.exclude` in `context.json`.

## Logs
Reduce them to the primary error, meaningful stack frames, relevant warnings, the reproduction command and relevant state (≤ `log_reduction.max_lines`).

## Caching
When a worker supports prefix caching, order input stable-first: protocol, project config, constraints. Put dynamic material after it: task, retrieved code, errors, diff. The `stability` field in `context.json` marks which is which. Correctness must never depend on caching.
