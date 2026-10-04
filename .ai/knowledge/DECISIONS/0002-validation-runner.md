# 0002: Run canonical validation with `tools/ai-validate`

- Status: accepted
- Date: 2026-10-04

## Context
Protocol 1.0.0 made `confirmed: true` commands in `project.json` the canonical validation, but shipped no runner. Projects therefore re-listed the same commands in Makefiles or scripts, which drift from `project.json`, and workers reported "passed" when a command was silently not run because a tool (Docker, a compiler) was missing.

## Decision
Add `tools/ai-validate`, which runs `ai-check` and then the confirmed commands from `project.json`. Commands may declare `requires` (executables), `cwd` and `timeout_s`. A command whose requirements are missing is reported **SKIP**, never PASS. Protocol §7 names the tool. Protocol version 1.1.0.

## Alternatives considered
- Leave running to each project: rejected; the drift and silent-skip problems recur in every project.
- Run unconfirmed commands too: rejected; inferred commands are evidence, not instructions.

## Consequences
- Wrappers (`make check`, `./dev check`) call `ai-validate` rather than keeping their own command lists.
- Reports (`--json`, `--report`) are usable as validation evidence in result packets.
- Reversal: if a project needs a DAG of validation steps with dependencies, revisit this flat ordered list.
