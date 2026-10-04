# 0001: Adopt the portable AI project protocol

- Status: accepted

## Context
Work on this project may be done by different AI models, providers, agent frameworks and humans over time. Knowledge held in chat history or vendor-specific memory is lost when the worker changes.

## Decision
The repository owns all project knowledge, as defined by `AI_PROTOCOL.md`:
- Canonical state is JSON validated by schemas in `.ai/schemas/`.
- Human knowledge lives in Markdown under `.ai/knowledge/`.
- Routing is by capability, never by model name.
- Provider files (`AGENTS.md`, `CLAUDE.md`, …) are thin shims.

## Alternatives considered
- Vendor-specific memory/instructions only: rejected because they are not portable.
- YAML state: rejected because it cannot be parsed with the guaranteed runtime (Python stdlib).

## Consequences
- Every finished task leaves a result packet and updated state.
- New models participate by registering in `.ai/runtime/` without changes to canonical config.
- `tools/ai-check` must pass before work is considered complete.
