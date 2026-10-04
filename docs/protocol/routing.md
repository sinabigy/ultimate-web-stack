# Capability routing

## Why capabilities, not models
Model names, prices and rankings change faster than projects do. The canonical files say **what a task needs**. A local, non-authoritative registry says **what is available today**. Only the registry changes when a new model appears.

## Canonical files (tracked)
- `.ai/config/capabilities.json` defines:
  - The core capability vocabulary (`core.*`).
  - The tiers:
    - 0 deterministic
    - 1 economical
    - 2 primary
    - 3 highest reasoning
  - Ordered class vocabularies: reasoning, cost, latency, context.
- `.ai/config/routing.json` defines:
  - Task classes. Each route has only `min_tier`, classes, `required_capabilities`, `preferred_capabilities` and `tools_required`.
  - Risk-based minimum tiers.
  - Escalation triggers.

`routing.schema.json` sets `additionalProperties: false` on routes, so a field naming a model cannot validate. Vendor independence is enforced by structure, not by scanning text for brand names.

## Extensible capabilities
A capability ID is namespaced: `namespace.name`.
- `core.*` is the shared vocabulary.
- `future.*`, `vendor.*`, `local.*` or any other namespace is valid.

Unknown capabilities are preserved and reported as INFO by `ai-check`, never rejected. Extend the class vocabularies by appending. Never rename or reorder existing values.

## Resolving a route
Run `python3 tools/ai-route T-NNNN`. It works out:
- `min_tier = max(route.min_tier, risk_min_tier[task.risk])`
- required capabilities = route ∪ task

If the tier is 0, no model is needed. If `.ai/runtime/available-models.json` exists, qualifying workers are listed cheapest-first. Without a registry you still get the requirements. A malformed registry makes `ai-route` fail cleanly, but the canonical project stays valid.

## Choosing a tier
Use the lowest tier likely to succeed reliably.
- **Tier 1:** classification, extraction, docs, boilerplate, simple fixes.
- **Tier 2:** normal features, debugging, multi-file changes, integrations, refactors.
- **Tier 3:** architecture, unknown hard bugs, concurrency, distributed systems, security, complex migrations, contradictory evidence, repeated failures.

## Escalation
The triggers are listed in `routing.json → escalation`. Before escalating, name the missing capability. Changing strategy at the same tier is often cheaper than escalating. After the hard part is done, de-escalate routine follow-up work.

## Subagents
Use a subagent only when it isolates a large investigation, enables real parallelism, keeps the main context clean, or needs specialist capability, and only if total cost goes down.
- Give it only what it needs.
- Require a compact handoff: conclusion, evidence, files, recommendation, risks, validation and open questions. The result schema has the same shape.
