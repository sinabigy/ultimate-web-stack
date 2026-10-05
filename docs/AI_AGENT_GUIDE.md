# Working with AI coding agents

This repository, and every project generated from it, is built so that a coding agent can
continue the work **from the repository alone**, without the original conversation.

## What the agent finds
| file | what it answers |
|---|---|
| `AI_PROTOCOL.md` | how work is done here: authority order, task workflow, validation, budget (short; the agent reads it first) |
| `.ai/config/project.json` | objective, stack, enabled modules (`architecture`), the confirmed validation commands |
| `.ai/state/` | `STATUS.md` (the eight state questions), `ROADMAP.json`, `CURRENT_TASK.json` |
| `.ai/tasks/` | task packets with acceptance criteria and evidence; `tools/ai-task` enforces their lifecycle |
| `.ai/knowledge/` | architecture map, constraints and security invariants, **conventions**, known issues, lessons, ADRs |
| `docs/authorization/README.md` | the step-by-step guide to adding a permission and protecting an endpoint |

Tool-specific shims (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`) only point to the protocol; they hold
no project facts. The protocol is model-independent: it routes work by capability, never by
model name.

## How to brief an agent
Give it the repository and the product request, nothing else:

> Read `AI_PROTOCOL.md` and follow it. The objective: «your feature request». Prove it with
> `./dev check` and a live check against `./dev up`, then record the evidence with
> `tools/ai-task complete`.

The protocol makes the agent:
1. check the state (`python3 tools/ai-check`);
2. record your request as the objective;
3. work through one task at a time;
4. validate with the project's confirmed commands;
5. update the durable knowledge for the next session.

## Evidence: the clean-room runs
Two agents were given only a generated repository and a request:

| run | request | result |
|---|---|---|
| 1 | organization-scoped Projects: CRUD, permissions, audit, UI | `./dev check` 23/23; member 403 and cross-tenant 404 shown live; nothing read outside the repo |
| 2 | organization Announcements, with notifications to members | `./dev check` 22/22 (including release image and live systemd); member writes 403 (audited), cross-tenant 404 by slug and id, notifications only to the other members; scorecard "yes" on all 11 capabilities |

Every reusable gap they reported was fixed in the blueprint, about 14 in all. Examples: an
authorization recipe, a conventions file, undocumented notification and navigation patterns, and
a real logout race condition. Details: [FINAL_REPORT.md](FINAL_REPORT.md#clean-room-findings).

## What still needs a human
- **Confirming the objective:** the discovery task asks the user; an agent records the request as
  its statement.
- **Production secrets, identity provider setup and anything outward-facing:** deploying,
  publishing, sending.
- **Review:** agents run the same validation as CI, but a human approves merges.
