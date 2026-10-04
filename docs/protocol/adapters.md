# Adapters: shims and runtime registries

## Provider shims
`AGENTS.md`, `CLAUDE.md` and `GEMINI.md` (or any future equivalent) are **compatibility adapters**. They exist so that tools which auto-read a particular filename find the protocol.

They are listed in `project.json → shims`. `ai-check` enforces that each listed shim:
- exists;
- points to `AI_PROTOCOL.md` and `.ai/state/CURRENT_TASK.json`.

It *warns* (never fails) when a shim:
- grows large (`policies.max_shim_lines_warning`);
- repeats protocol lines.

To add a shim, create the file with the pointer text and add it to `shims`. To remove one, delete the file and the list entry. Canonical state doesn't change either way. A shim may carry a few tool-specific notes, such as an import syntax. Project facts never go in a shim.

## Runtime registry (local, non-authoritative)
`.ai/runtime/` is gitignored. Deleting it completely leaves the project valid. It may hold:
- `available-models.json`: workers available in this environment (schema `runtime-registry.schema.json`). Start from `.ai/templates/available-models.example.json`.
- `provider-capabilities.json`, `session.json`: free-form local adapter data.
- `usage.jsonl`: optional telemetry (`usage.schema.json`).

A registry entry looks like this:
```json
{"id": "acme-coder-9", "provider": "acme", "max_tier": 3,
 "capabilities": ["core.code_generation", "core.reasoning", "future.native_repo_reasoning"],
 "reasoning_class": "deep", "cost_class": "medium", "latency_class": "normal", "context_class": "very_large"}
```
Unknown fields and capabilities are preserved. A model released years from now participates by adding an entry here. No canonical file changes.

## What is deliberately not here
The template ships no model client, orchestration runtime, API gateway or provider SDK. These can be built later behind this protocol as adapters that read `ai-route` output and the registry.
