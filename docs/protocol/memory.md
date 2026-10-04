# Memory

Project memory is **durable, compressed knowledge**. It is not a transcript archive.

| Store | Contents |
|---|---|
| `.ai/knowledge/LESSONS.md` | Verified lessons: statement, evidence, scope |
| `.ai/knowledge/DECISIONS/` | ADRs, only for decisions with future consequences |
| `.ai/knowledge/KNOWN_ISSUES.md` | Open problems with repro and impact |
| `.ai/knowledge/CONSTRAINTS.md` | Hard constraints and their sources |
| `.ai/knowledge/CONVENTIONS.md` | Conventions that are not obvious from the code |
| `.ai/knowledge/ARCHITECTURE.md` | One line per component |
| `.ai/knowledge/<topic>/` | Optional topic folders (e.g. `performance/`, `providers/`) when a flat file would grow too large. Index them from `ARCHITECTURE.md` or `project.json → architecture.knowledge`. |
| `.ai/context/summaries/*.result.json` | Per-task handoff packets (history; not loaded by default) |
| `.ai/state/STATUS.md` | Answers to the eight state questions |

**Store:** decisions, constraints, recurring failures, important discoveries, blockers, verified lessons and critical external assumptions.

**Never store:** conversations, raw command output, temporary hypotheses, duplicated docs, things trivially rediscovered from the source, or secrets.

Bad: "During a previous conversation we tried several approaches to networking…"
Good:
```
LESSON: Server movement must use a fixed simulation timestep.
EVIDENCE: MultiplayerReplicationTest fails with variable timestep (divergence).
APPLIES TO: server/sim
```

## Compaction
At task boundaries, keep what changed, why, decisions, validation, unresolved issues, new constraints and references. Discard chatter, failed hypotheses that teach nothing, duplicate context and raw logs.

## Vendor memory
Provider memory features may be used as a cache. They must never be the only copy of project knowledge. If it matters, it goes into the repository.

## Usage telemetry (optional)
`.ai/schemas/usage.schema.json` defines one JSON line per task attempt in `.ai/runtime/usage.jsonl`. That file is gitignored. Each record covers:
- tier, and optionally provider and model
- context, output and cached usage
- cost
- tool calls, retries, subagents
- duration
- validation result and success

Every metric except tool calls and retries is optional. Aggregate results deliberately (`.ai/evals/benchmarks/`). Promote conclusions into LESSONS, or into routing and budgets via an ADR.
